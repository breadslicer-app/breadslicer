//! The support settings a person edits (BS-70): the knobs DragonFruit's own
//! auto-support panel exposes, plus the raft and the tube style, as one
//! serializable value — what a preset saves.
//!
//! Every value is clamped to the bounds DragonFruit declares for it
//! (`auto_support_constraints`), so a hand-edited preset cannot push the
//! planner outside what it was built for.

use serde::{Deserialize, Serialize};
use support_gen_df::features::slicing::smooth_shafts::{ShaftStyle, SmoothShaftParams};
use support_gen_df::native::islands::body::IslandBodyScan;
use support_gen_df::pipeline::PipelineSettings;
use support_gen_df::supports::auto_support::auto_place::helpers::apply_zone_size;
use support_gen_df::supports::auto_support::settings::{SizingPreset, auto_support_constraints};
use support_gen_df::supports::auto_support::zone_rules::{STABILIZATION_MAX_ANGLE_DEG, ZoneRules};
use support_gen_df::supports::rafts::crenelated::geometry::compute_raft_footprint::raft_band_top_mm;
use support_gen_df::supports::rafts::crenelated::raft_types::RaftBottomMode;

use crate::zone_presets::ZonePreset;

/// How high a model is lifted off the plate to leave room for its supports
/// under it, in millimetres: the usual automatic lift of resin slicers, and
/// what the command line's `--df-lift` defaults to. A model lifted only
/// above the raft leaves the supports no room to grow under its lowest
/// overhangs (#131).
pub const DEFAULT_SUPPORT_LIFT_MM: f64 = 5.0;

/// The lift to recommend for a model that stands in the raft: the default
/// support lift, or the lift a person configured when it is higher.
pub fn recommended_lift_mm(configured_mm: Option<f64>) -> f64 {
    configured_mm
        .filter(|mm| mm.is_finite())
        .map_or(DEFAULT_SUPPORT_LIFT_MM, |mm| {
            mm.max(DEFAULT_SUPPORT_LIFT_MM)
        })
}

/// How the raft's bottom is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RaftMode {
    #[default]
    Solid,
    Lines,
    Off,
}

/// What a person sets for an auto-support run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DfSupportOptions {
    /// The size tier tips, shafts and roots are drawn from: `detail`,
    /// `structure` or `anchor` (DragonFruit's factory Support Studio bands).
    pub sizing: String,
    /// Projected surface each grid support carries, mm²: smaller is denser.
    pub area_per_support_mm2: f64,
    /// Faces flatter than this (degrees from horizontal) need support.
    pub overhang_angle_deg: f64,
    /// Multiplier over the sizing tier.
    pub size_scale: f64,
    /// Islands smaller than this, mm², are left alone.
    pub min_island_area_mm2: f64,
    /// How much of each overhang region the grid must cover, percent.
    pub coverage_percent: f64,
    /// Stabilization anchors for poses resting on a point or an edge.
    pub stabilization: bool,
    /// Cross-bracing between supports.
    pub bracing: bool,
    /// Continuous tubes (SoulCrafted) rather than DragonFruit's segments and
    /// spheres.
    pub smooth_shafts: bool,
    pub raft: RaftMode,
    /// Raft plate thickness, mm.
    pub raft_thickness_mm: f64,
    /// The crenelated wall around a solid raft.
    pub raft_wall: bool,
    /// A zone preset (#360): when set, it sizes and spaces every support by
    /// the zone it lands in, and its bracing replaces the default; sizing,
    /// density, overhang angle, size scale, smallest island, coverage and
    /// stabilization above are not read. The raft is still the raft above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zones: Option<ZonePreset>,
}

impl Default for DfSupportOptions {
    fn default() -> Self {
        let c = &auto_support_constraints::AREA_PER_SUPPORT_MM2;
        DfSupportOptions {
            sizing: SizingPreset::Structure.as_str().to_owned(),
            area_per_support_mm2: c.default_value,
            overhang_angle_deg: auto_support_constraints::OVERHANG_SELF_SUPPORT_ANGLE_DEG
                .default_value,
            size_scale: auto_support_constraints::SIZE_SCALE.default_value,
            min_island_area_mm2: auto_support_constraints::MIN_ISLAND_AREA_MM2.default_value,
            coverage_percent: auto_support_constraints::COVERAGE_TARGET_PERCENT.default_value,
            stabilization: true,
            bracing: true,
            smooth_shafts: true,
            raft: RaftMode::Solid,
            raft_thickness_mm: 0.5,
            raft_wall: true,
            zones: None,
        }
    }
}

fn clamp(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl DfSupportOptions {
    /// The options with every value inside its bounds and a known sizing tier.
    pub fn clamped(&self) -> Self {
        use auto_support_constraints as c;
        let d = DfSupportOptions::default();
        let sizing = if SizingPreset::ALL.iter().any(|p| p.as_str() == self.sizing) {
            self.sizing.clone()
        } else {
            d.sizing.clone()
        };
        let bound =
            |v: f64, k: &support_gen_df::supports::numeric_constraint::NumericConstraint| {
                clamp(v, k.min, k.max, k.default_value)
            };
        DfSupportOptions {
            sizing,
            area_per_support_mm2: bound(self.area_per_support_mm2, &c::AREA_PER_SUPPORT_MM2),
            overhang_angle_deg: bound(self.overhang_angle_deg, &c::OVERHANG_SELF_SUPPORT_ANGLE_DEG),
            size_scale: bound(self.size_scale, &c::SIZE_SCALE),
            min_island_area_mm2: bound(self.min_island_area_mm2, &c::MIN_ISLAND_AREA_MM2),
            coverage_percent: bound(self.coverage_percent, &c::COVERAGE_TARGET_PERCENT).round(),
            raft_thickness_mm: clamp(self.raft_thickness_mm, 0.1, 3.0, d.raft_thickness_mm),
            zones: self.zones.as_ref().map(|z| ZonePreset {
                rules: z.rules.clamped(),
                auto_lift_mm: clamp(z.auto_lift_mm, 0.0, 50.0, DEFAULT_SUPPORT_LIFT_MM),
                bracing: z.bracing.clone(),
                grid_mm: z.grid_mm.map(|mm| clamp(mm, 1.0, 50.0, 7.0)),
            }),
            ..self.clone()
        }
    }

    /// How high a model should stand above the plate for these supports:
    /// the zone preset's auto-lift, else the default support lift.
    pub fn lift_mm(&self) -> f64 {
        self.zones
            .as_ref()
            .map_or(DEFAULT_SUPPORT_LIFT_MM, |z| z.auto_lift_mm)
    }

    /// The self-support angle these options classify overhangs by, degrees
    /// from horizontal (0° = flat ceiling): faces flatter than it need
    /// support. A zone preset's rule reads from vertical; this is that angle
    /// turned around, the number [`Self::apply`] hands the scans.
    pub fn self_support_angle_deg(&self) -> f64 {
        let o = self.clamped();
        o.zones.as_ref().map_or(o.overhang_angle_deg, |z| {
            zone_self_support_angle_deg(&z.rules)
        })
    }

    /// Writes these options into `settings`.
    pub fn apply(&self, settings: &mut PipelineSettings) {
        let o = self.clamped();
        let auto = &mut settings.supports.auto_support;
        auto.sizing_preset = o.sizing.clone();
        auto.area_per_support_mm2 = o.area_per_support_mm2;
        auto.overhang_self_support_angle_deg = o.overhang_angle_deg;
        auto.size_scale = o.size_scale;
        auto.min_island_area_mm2 = o.min_island_area_mm2;
        auto.coverage_target_percent = o.coverage_percent;
        auto.stabilization_enabled = o.stabilization;
        auto.debug_skip_auto_bracing = !o.bracing;
        settings.shaft_style = if o.smooth_shafts {
            ShaftStyle::Smooth(SmoothShaftParams::default())
        } else {
            ShaftStyle::DragonFruit
        };
        let mode = match o.raft {
            RaftMode::Solid => RaftBottomMode::Solid,
            RaftMode::Lines => RaftBottomMode::Line,
            RaftMode::Off => RaftBottomMode::Off,
        };
        if let Some(zones) = &o.zones {
            apply_zones(zones, settings);
        }
        // Through the store, as DragonFruit's panel does, only when the raft
        // really changes: the store also resolves the rest of the raft.
        let g = settings.rafts.global;
        if g.bottom_mode != mode
            || g.thickness != o.raft_thickness_mm
            || g.wall_enabled != o.raft_wall
        {
            settings.rafts.update_global(
                |r| {
                    r.bottom_mode = mode;
                    r.thickness = o.raft_thickness_mm;
                    r.wall_enabled = o.raft_wall;
                },
                true,
            );
        }
    }
}

/// The self-support angle a zone preset's rules classify by, degrees from
/// horizontal, inside the bounds the scans accept.
fn zone_self_support_angle_deg(rules: &ZoneRules) -> f64 {
    rules.self_support_angle_deg().clamp(
        auto_support_constraints::OVERHANG_SELF_SUPPORT_ANGLE_DEG.min,
        auto_support_constraints::OVERHANG_SELF_SUPPORT_ANGLE_DEG.max,
    )
}

/// Writes a zone preset into `settings` (#360): its rules, the overhang
/// angle the scans classify by, its default size for supports placed by
/// hand, the scans its zones read, its bracing and its sizes drawn exactly.
fn apply_zones(zones: &ZonePreset, settings: &mut PipelineSettings) {
    let rules = zones.rules.clamped();
    let supports = &mut settings.supports;
    let auto = &mut supports.auto_support;
    auto.overhang_self_support_angle_deg = zone_self_support_angle_deg(&rules);
    // The sizes are drawn as written: no model scaling.
    auto.model_scale_enabled = false;
    auto.size_scale = 1.0;
    // The zones stand in for DragonFruit's pose anchors and minima rings.
    auto.stabilization_enabled = false;
    auto.minima_reinforcement_enabled = false;
    auto.zone_rules = Some(rules.clone());
    if let Some(i) = rules.default_size_index() {
        apply_zone_size(supports, &rules, i);
    }
    supports.dev_tools.max_lateral_mm = rules.search_ground_mm;
    if let Some(spacing_mm) = zones.grid_mm {
        supports.grid.enabled = true;
        supports.grid.spacing_mm = spacing_mm;
        supports.grid.pillars = true;
        // A pillar takes all that hangs from its node.
        supports.auto_support.max_attachments_per_trunk = f64::MAX;
    }
    zones.bracing.apply(&mut supports.auto_bracing);
    settings.tip_drawing.per_support_penetration = true;
    settings.scan.island_bodies = Some(IslandBodyScan::new(
        &rules.section_heights_mm(),
        rules.volume_cap_mm3(),
    ));
    settings.scan.steep_angle_deg = rules
        .stabilization
        .enabled
        .then_some(STABILIZATION_MAX_ANGLE_DEG as f32);
    settings.panel.keep_covered_voxel_islands = true;
}

impl DfSupportOptions {
    /// The height of the raft's top above the plate, in millimetres: its
    /// plate (or line beams) plus the crenelated wall, the material a model
    /// must stand above to clear it (#131). `None` with the raft off.
    pub fn raft_top_mm(&self) -> Option<f64> {
        if self.clamped().raft == RaftMode::Off {
            return None;
        }
        let mut settings = PipelineSettings::defaults(0.05);
        self.apply(&mut settings);
        let top = raft_band_top_mm(&settings.rafts.global);
        (top.is_finite() && top > 0.0).then_some(top)
    }

    /// The layers of `layer_height_mm` the raft takes from the plate up
    /// ([`Self::raft_top_mm`] rounded up to whole layers); 0 with the raft
    /// off or a layer height that is not positive.
    pub fn raft_layers(&self, layer_height_mm: f64) -> usize {
        match self.raft_top_mm() {
            Some(top) if layer_height_mm.is_finite() && layer_height_mm > 0.0 => {
                // A hair under a whole layer counts as that layer.
                (top / layer_height_mm - 1e-6).ceil().max(0.0) as usize
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod options_tests {
    use super::*;

    #[test]
    fn defaults_change_nothing() {
        let base = PipelineSettings::defaults(0.05);
        let mut applied = base.clone();
        DfSupportOptions::default().apply(&mut applied);
        assert_eq!(
            applied.supports.auto_support, base.supports.auto_support,
            "the defaults are DragonFruit's"
        );
        assert_eq!(applied.rafts.global, base.rafts.global);
    }

    #[test]
    fn the_self_support_angle_is_the_one_the_scans_get() {
        let cases = [
            DfSupportOptions::default(),
            DfSupportOptions {
                overhang_angle_deg: 30.0,
                ..Default::default()
            },
            // Past the bounds: both read the clamped value.
            DfSupportOptions {
                overhang_angle_deg: 89.0,
                ..Default::default()
            },
            // A zone preset's rule reads from vertical: 38° is 52° here.
            DfSupportOptions {
                zones: Some(crate::zone_presets::minis_30()),
                ..Default::default()
            },
            DfSupportOptions {
                zones: Some(crate::zone_presets::figures_70()),
                ..Default::default()
            },
            DfSupportOptions {
                zones: Some(crate::zone_presets::figures_150()),
                ..Default::default()
            },
        ];
        for options in cases {
            let mut settings = PipelineSettings::defaults(0.05);
            options.apply(&mut settings);
            assert_eq!(
                options.self_support_angle_deg(),
                settings
                    .supports
                    .auto_support
                    .overhang_self_support_angle_deg,
                "{options:?}"
            );
        }
        let minis = DfSupportOptions {
            zones: Some(crate::zone_presets::minis_30()),
            ..Default::default()
        };
        assert_eq!(minis.self_support_angle_deg(), 52.0);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let wild = DfSupportOptions {
            area_per_support_mm2: 1000.0,
            overhang_angle_deg: f64::NAN,
            sizing: "nonsense".into(),
            ..Default::default()
        }
        .clamped();
        assert_eq!(wild.area_per_support_mm2, 30.0);
        assert_eq!(wild.overhang_angle_deg, 45.0);
        assert_eq!(wild.sizing, "structure");
    }

    #[test]
    fn a_preset_round_trips_through_json() {
        let o = DfSupportOptions {
            sizing: "detail".into(),
            raft: RaftMode::Lines,
            ..Default::default()
        };
        let text = serde_json::to_string(&o).unwrap();
        assert_eq!(serde_json::from_str::<DfSupportOptions>(&text).unwrap(), o);
    }

    #[test]
    fn the_raft_top_is_its_plate_and_its_wall() {
        let solid = DfSupportOptions::default();
        let top = solid.raft_top_mm().expect("a solid raft");
        assert!((top - 0.85).abs() < 1e-9, "{top}");
        assert_eq!(solid.raft_layers(0.05), 17);
        assert_eq!(solid.raft_layers(0.1), 9);
        let no_wall = DfSupportOptions {
            raft_wall: false,
            ..Default::default()
        };
        assert!((no_wall.raft_top_mm().unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(no_wall.raft_layers(0.05), 10);
        let off = DfSupportOptions {
            raft: RaftMode::Off,
            ..Default::default()
        };
        assert_eq!(off.raft_top_mm(), None);
        assert_eq!(off.raft_layers(0.05), 0);
        assert_eq!(solid.raft_layers(0.0), 0);
    }

    #[test]
    fn the_recommended_lift_is_the_default_or_a_higher_configured_one() {
        assert_eq!(recommended_lift_mm(None), DEFAULT_SUPPORT_LIFT_MM);
        assert_eq!(recommended_lift_mm(Some(0.0)), DEFAULT_SUPPORT_LIFT_MM);
        assert_eq!(recommended_lift_mm(Some(f64::NAN)), DEFAULT_SUPPORT_LIFT_MM);
        assert_eq!(recommended_lift_mm(Some(8.0)), 8.0);
    }
}

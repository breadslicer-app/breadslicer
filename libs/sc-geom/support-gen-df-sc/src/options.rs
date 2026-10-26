//! The support settings a person edits (SC-86): the knobs DragonFruit's own
//! auto-support panel exposes, plus the raft and the tube style, as one
//! serializable value — what a preset saves.
//!
//! Every value is clamped to the bounds DragonFruit declares for it
//! (`auto_support_constraints`), so a hand-edited preset cannot push the
//! planner outside what it was built for.

use serde::{Deserialize, Serialize};
use support_gen_df::features::slicing::smooth_shafts::{ShaftStyle, SmoothShaftParams};
use support_gen_df::pipeline::PipelineSettings;
use support_gen_df::supports::auto_support::settings::{SizingPreset, auto_support_constraints};
use support_gen_df::supports::rafts::crenelated::raft_types::RaftBottomMode;

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
            ..self.clone()
        }
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
}

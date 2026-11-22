//! The support settings a person edits (BS-70): the preset, bracing, the
//! raft and the tube style, as one serializable value (what a preset saves).
//!
//! Every value is clamped to its bounds (`auto_support_constraints`), so a
//! hand-edited preset cannot push the planner outside what it was built for.

use serde::{Deserialize, Serialize};
use support_mesh::features::slicing::smooth_shafts::{ShaftStyle, SmoothShaftParams};
use support_mesh::rafts::crenelated::geometry::compute_raft_footprint::raft_band_top_mm;
use support_mesh::rafts::crenelated::raft_types::{RaftBottomMode, RaftSettings};
use support_pipeline::PipelineSettings;

use crate::zone_presets::{ZonePreset, apply_zone_preset, zone_self_support_angle_deg};

/// How high a model is lifted off the plate to leave room for its supports
/// under it, in millimetres: the usual automatic lift of resin slicers, and
/// what the command line's `--support-lift` defaults to. A model lifted only
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
pub struct SupportOptions {
    /// Cross-bracing between supports.
    pub bracing: bool,
    /// Continuous tubes rather than segments and spheres.
    pub smooth_shafts: bool,
    pub raft: RaftMode,
    /// Raft plate thickness, mm.
    pub raft_thickness_mm: f64,
    /// The crenelated wall around a solid raft.
    pub raft_wall: bool,
    /// SoulCrafted (#138): holes this wide through a solid raft between the
    /// supports, mm; 0 for none.
    #[serde(skip_serializing_if = "is_zero")]
    pub raft_holes_mm: f64,
    /// SoulCrafted (#138): a pad under each support instead of one solid raft.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub raft_pads: bool,
    /// SoulCrafted (#99): tabs off a solid raft's two ends, to lever it off.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub raft_tabs: bool,
    /// One raft for all (#556): a raft of lines under every set standing on
    /// one, drawn once for the plate; a set's own drawing leaves it out.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub one_raft: bool,
    /// The zone preset (#360): it sizes and spaces every support by the
    /// zone it lands in, and its bracing replaces the default; the raft is
    /// still the raft above. Options saved without one (from the presets
    /// that had none) take Miniatures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zones: Option<ZonePreset>,
}

impl Default for SupportOptions {
    fn default() -> Self {
        SupportOptions {
            bracing: true,
            smooth_shafts: true,
            // A raft of lines (#435): as close to the supports, half the
            // first layer of a solid plate, and it peels off better.
            raft: RaftMode::Lines,
            raft_thickness_mm: 0.5,
            raft_wall: true,
            raft_holes_mm: 0.0,
            raft_pads: false,
            raft_tabs: false,
            one_raft: false,
            zones: Some(crate::zone_presets::minis_30()),
        }
    }
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn clamp(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl SupportOptions {
    /// The options with every value inside its bounds and a zone preset.
    pub fn clamped(&self) -> Self {
        let d = SupportOptions::default();
        SupportOptions {
            raft_thickness_mm: clamp(self.raft_thickness_mm, 0.1, 3.0, d.raft_thickness_mm),
            raft_holes_mm: if self.raft_holes_mm > 0.0 {
                clamp(self.raft_holes_mm, 0.5, 10.0, 0.0)
            } else {
                0.0
            },
            zones: Some(self.zone_preset()),
            ..self.clone()
        }
    }

    /// The zone preset, inside its bounds: Miniatures when the options were
    /// saved without one.
    pub fn zone_preset(&self) -> ZonePreset {
        let z = self
            .zones
            .clone()
            .unwrap_or_else(crate::zone_presets::minis_30);
        ZonePreset {
            rules: z.rules.clamped(),
            auto_lift_mm: clamp(z.auto_lift_mm, 0.0, 50.0, DEFAULT_SUPPORT_LIFT_MM),
            bracing: z.bracing,
            grid_mm: z.grid_mm.map(|mm| clamp(mm, 1.0, 50.0, 7.0)),
        }
    }

    /// How high a model should stand above the plate for these supports:
    /// the zone preset's auto-lift, else the default support lift.
    pub fn lift_mm(&self) -> f64 {
        self.zones
            .as_ref()
            .map_or(crate::zone_presets::minis_30().auto_lift_mm, |z| {
                z.auto_lift_mm
            })
    }

    /// The self-support angle these options classify overhangs by, degrees
    /// from horizontal (0° = flat ceiling): faces flatter than it need
    /// support. A zone preset's rule reads from vertical; this is that angle
    /// turned around, the number [`Self::apply`] hands the scans.
    pub fn self_support_angle_deg(&self) -> f64 {
        zone_self_support_angle_deg(&self.zone_preset().rules)
    }

    /// How far around itself a tip holds an overhang, mm (#484): the
    /// spacing these options keep between overhang tips. A zone preset
    /// samples an overhang that far apart and keeps no two of its points
    /// closer than 0.9 of it, so every point of the region lies within it of
    /// a tip.
    pub fn tip_reach_mm(&self) -> f64 {
        self.zone_preset().rules.overhang.spacing_mm
    }

    /// Writes these options into `settings`.
    pub fn apply(&self, settings: &mut PipelineSettings) {
        let o = self.clamped();
        settings.supports.auto_support.debug_skip_auto_bracing = !o.bracing;
        settings.shaft_style = if o.smooth_shafts {
            ShaftStyle::Smooth(SmoothShaftParams::default())
        } else {
            ShaftStyle::Segmented
        };
        apply_zone_preset(&o.zone_preset(), settings, o.one_raft);
        o.apply_raft(settings);
    }

    /// Sets the raft of `settings` to these options' (#519): its kind, its
    /// thickness and its wall. Whether that changed it.
    pub(crate) fn apply_raft(&self, settings: &mut PipelineSettings) -> bool {
        let o = self.clamped();
        let mode = match o.raft_drawn() {
            RaftMode::Solid => RaftBottomMode::Solid,
            RaftMode::Lines => RaftBottomMode::Line,
            RaftMode::Off => RaftBottomMode::Off,
        };
        // Only when the raft really changes.
        let apart = settings.raft_apart != o.one_raft;
        settings.raft_apart = o.one_raft;
        let g = settings.raft;
        if g.bottom_mode != mode
            || g.thickness != o.raft_thickness_mm
            || g.wall_enabled != o.raft_wall
            || g.perforation_mm != o.raft_holes_mm
            || g.pads != o.raft_pads
            || g.peel_tabs != o.raft_tabs
        {
            settings.raft = RaftSettings {
                bottom_mode: mode,
                thickness: o.raft_thickness_mm,
                wall_enabled: o.raft_wall,
                perforation_mm: o.raft_holes_mm,
                pads: o.raft_pads,
                peel_tabs: o.raft_tabs,
                ..g
            }
            .settled();
            return true;
        }
        apart
    }
}

impl SupportOptions {
    /// The height of the raft's top above the plate, in millimetres: its
    /// plate (or line beams) plus the crenelated wall, the material a model
    /// must stand above to clear it (#131). `None` with the raft off.
    /// The raft these options draw: one raft for all is a raft of lines
    /// (#556), whatever raft the preset keeps for when it goes off.
    pub fn raft_drawn(&self) -> RaftMode {
        if self.one_raft {
            RaftMode::Lines
        } else {
            self.raft
        }
    }

    pub fn raft_top_mm(&self) -> Option<f64> {
        if self.clamped().raft_drawn() == RaftMode::Off {
            return None;
        }
        let mut settings = PipelineSettings::defaults(0.05);
        self.apply(&mut settings);
        let top = raft_band_top_mm(&settings.raft);
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
    use support_model::auto_support::settings::AutoSupportSettings;
    use support_model::settings::types::GridSettings;

    #[test]
    fn defaults_change_nothing() {
        let base = PipelineSettings::defaults(0.05);
        let mut applied = base.clone();
        SupportOptions::default().apply(&mut applied);
        assert_eq!(
            applied.supports.auto_support, base.supports.auto_support,
            "the defaults change nothing"
        );
        // But the raft: a raft of lines (#435).
        assert_eq!(applied.raft.bottom_mode, RaftBottomMode::Line);
        let solid = SupportOptions {
            raft: RaftMode::Solid,
            ..Default::default()
        };
        let mut applied = base.clone();
        solid.apply(&mut applied);
        assert_eq!(applied.raft, base.raft);
    }

    #[test]
    fn the_self_support_angle_is_the_one_the_scans_get() {
        let cases = [
            SupportOptions::default(),
            // A zone preset's rule reads from vertical: 38° is 52° here.
            SupportOptions {
                zones: Some(crate::zone_presets::minis_30()),
                ..Default::default()
            },
            SupportOptions {
                zones: Some(crate::zone_presets::figures_70()),
                ..Default::default()
            },
            SupportOptions {
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
        let minis = SupportOptions {
            zones: Some(crate::zone_presets::minis_30()),
            ..Default::default()
        };
        assert_eq!(minis.self_support_angle_deg(), 52.0);
    }

    /// A zone preset's grid (#464) is the person's to turn off or space
    /// out: off, the scans get no grid and trunks keep their cap of members;
    /// on, its spacing, kept inside its bounds.
    #[test]
    fn a_zone_presets_grid_goes_off_or_takes_its_spacing() {
        let with = |grid_mm| SupportOptions {
            zones: Some(ZonePreset {
                grid_mm,
                ..crate::zone_presets::minis_30()
            }),
            ..Default::default()
        };
        let base = PipelineSettings::defaults(0.05);
        let mut off = base.clone();
        with(None).apply(&mut off);
        assert_eq!(off.supports.grid, GridSettings::default());
        assert_eq!(
            off.supports.auto_support.max_attachments_per_trunk,
            AutoSupportSettings::default().max_attachments_per_trunk
        );
        for (asked, spacing) in [(5.0, 5.0), (0.2, 1.0), (80.0, 50.0)] {
            let mut on = base.clone();
            with(Some(asked)).apply(&mut on);
            assert!(on.supports.grid.enabled && on.supports.grid.pillars);
            assert_eq!(on.supports.grid.spacing_mm, spacing, "{asked}");
        }
    }

    /// Under one raft for all the grid stays off (#576): the supports drop
    /// near their model instead of leaning out to pillars, whatever the
    /// preset's grid; the preset keeps it for when one raft for all goes off.
    #[test]
    fn under_one_raft_for_all_the_grid_stays_off() {
        let base = PipelineSettings::defaults(0.05);
        let minis = SupportOptions {
            zones: Some(crate::zone_presets::minis_30()),
            ..Default::default()
        };
        assert!(minis.zones.as_ref().unwrap().grid_mm.is_some());
        let mut alone = base.clone();
        minis.apply(&mut alone);
        assert!(alone.supports.grid.enabled);
        let mut shared = base.clone();
        SupportOptions {
            one_raft: true,
            ..minis
        }
        .apply(&mut shared);
        assert_eq!(shared.supports.grid, GridSettings::default());
        assert_eq!(
            shared.supports.auto_support.max_attachments_per_trunk,
            AutoSupportSettings::default().max_attachments_per_trunk
        );
    }

    /// A tip holds the overhang around it as far as its options space
    /// overhang tips (#484): its zone preset's overhang spacing; options
    /// without zones are Miniatures'.
    #[test]
    fn a_tip_reaches_as_far_as_its_options_space_overhang_tips() {
        let zoned = |preset: ZonePreset| SupportOptions {
            zones: Some(preset),
            ..Default::default()
        };
        assert_eq!(zoned(crate::zone_presets::minis_30()).tip_reach_mm(), 1.8);
        assert_eq!(zoned(crate::zone_presets::figures_70()).tip_reach_mm(), 2.4);
        assert_eq!(
            zoned(crate::zone_presets::figures_150()).tip_reach_mm(),
            2.8
        );
        let without = SupportOptions {
            zones: None,
            ..Default::default()
        };
        assert_eq!(without.tip_reach_mm(), 1.8);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let wild = SupportOptions {
            raft_thickness_mm: f64::NAN,
            raft_holes_mm: 99.0,
            zones: None,
            ..Default::default()
        }
        .clamped();
        assert_eq!(wild.raft_thickness_mm, 0.5);
        assert_eq!(wild.raft_holes_mm, 10.0);
        assert_eq!(wild.zones, Some(SupportOptions::default().zone_preset()));
    }

    #[test]
    fn a_preset_round_trips_through_json() {
        let o = SupportOptions {
            raft: RaftMode::Solid,
            ..Default::default()
        };
        let text = serde_json::to_string(&o).unwrap();
        assert_eq!(serde_json::from_str::<SupportOptions>(&text).unwrap(), o);
    }

    #[test]
    fn the_raft_top_is_its_plate_and_its_wall() {
        let solid = SupportOptions {
            raft: RaftMode::Solid,
            ..Default::default()
        };
        let top = solid.raft_top_mm().expect("a solid raft");
        assert!((top - 0.85).abs() < 1e-9, "{top}");
        assert_eq!(solid.raft_layers(0.05), 17);
        assert_eq!(solid.raft_layers(0.1), 9);
        let no_wall = SupportOptions {
            raft_wall: false,
            ..solid.clone()
        };
        assert!((no_wall.raft_top_mm().unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(no_wall.raft_layers(0.05), 10);
        let off = SupportOptions {
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

//! Zone rules (SoulCrafted, not a DragonFruit file, #360): a support preset
//! that picks the size of every support by the zone it lands in, with sizes
//! drawn exactly as they are written.
//!
//! - **Island first layer:** an island's first layer, by its area: the band
//!   with the largest minimum area the island reaches sets the size and the
//!   spacing of the supports over that layer.
//! - **Island reinforcement:** extra supports around an island, by the volume
//!   of the island's own body (the model that grows from it until it joins
//!   the rest), on its flank at a height above its first layer.
//! - **Overhang:** faces that lean past an angle, over a least area.
//! - **Stabilization:** the down-facing surface the other zones leave, over a
//!   least area, at a sparse spacing.
//!
//! A size is the whole support: tip cone, trunk, base and the tip a support
//! standing on the model ends in. Nothing scales it: not the model's size,
//! not its weight, not the trunk (a tip is never widened to a share of it).

use serde::{Deserialize, Serialize};

use super::settings::SizingBand;

/// One support size, every number in millimetres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SupportSize {
    /// What the bands call it (`"small"`).
    pub id: String,
    /// The diameter where the tip touches the model surface.
    pub tip_diameter_mm: f64,
    /// The tip cone, from the surface to the trunk.
    pub tip_length_mm: f64,
    /// How far the tip sinks into the model past the surface.
    pub penetration_mm: f64,
    /// A neck of the tip diameter this long between the surface and the
    /// cone, where the support breaks off; 0 for none.
    pub break_point_mm: f64,
    /// The trunk, a cylinder.
    pub trunk_diameter_mm: f64,
    /// The disk the support stands on.
    pub base_diameter_mm: f64,
    pub base_thickness_mm: f64,
    /// The cone from the base disk up to the trunk; 0 for none.
    pub join_cone_mm: f64,
    /// The tip a support standing on the model ends in, below; `None` ends
    /// it in the same tip as the top.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_tip: Option<BaseTip>,
}

impl Default for SupportSize {
    fn default() -> Self {
        SupportSize {
            id: String::new(),
            tip_diameter_mm: 0.3,
            tip_length_mm: 2.3,
            penetration_mm: 0.0,
            break_point_mm: 0.0,
            trunk_diameter_mm: 1.1,
            base_diameter_mm: 5.0,
            base_thickness_mm: 0.25,
            join_cone_mm: 0.4,
            base_tip: None,
        }
    }
}

/// The lower tip of a support that stands on the model.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseTip {
    pub length_mm: f64,
    pub diameter_mm: f64,
}

/// An island first-layer band: islands whose first layer covers at least
/// `min_area_mm2` take `size`, `spacing_mm` apart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AreaBand {
    pub enabled: bool,
    pub min_area_mm2: f64,
    pub spacing_mm: f64,
    pub size: String,
}

impl Default for AreaBand {
    fn default() -> Self {
        AreaBand {
            enabled: true,
            min_area_mm2: 0.0,
            spacing_mm: 2.0,
            size: String::new(),
        }
    }
}

/// An island reinforcement band: islands whose body holds at least
/// `min_volume_mm3` take `size` around their flank, `height_mm` above their
/// first layer, `spacing_mm` apart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VolumeBand {
    pub enabled: bool,
    pub min_volume_mm3: f64,
    pub height_mm: f64,
    pub spacing_mm: f64,
    pub size: String,
}

impl Default for VolumeBand {
    fn default() -> Self {
        VolumeBand {
            enabled: true,
            min_volume_mm3: 0.1,
            height_mm: 1.5,
            spacing_mm: 2.0,
            size: String::new(),
        }
    }
}

/// Islands under both limits are left alone: specks the next layers join at
/// once.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IgnoreSmallIslands {
    pub enabled: bool,
    /// An island gathering less resin than this before it joins the rest
    /// (mm³).
    pub volume_mm3: f64,
    /// An island that joins the rest within this height, its layers included
    /// (mm): 0.10 at 0.05 mm layers ignores islands of up to two layers
    /// (#406).
    pub height_mm: f64,
}

/// Slack on the island height comparison (mm): a body's height is its
/// layers times the layer height, in single precision.
const ISLAND_HEIGHT_TOLERANCE_MM: f64 = 1e-4;

impl IgnoreSmallIslands {
    /// Whether an island whose body gathers `volume_mm3` and joins the rest
    /// within `height_mm` (its layers times the layer height) is left alone:
    /// under the volume, and within the height, its last layer included
    /// (#406).
    pub fn ignores(&self, volume_mm3: f64, height_mm: f64) -> bool {
        self.enabled
            && volume_mm3 < self.volume_mm3
            && height_mm <= self.height_mm + ISLAND_HEIGHT_TOLERANCE_MM
    }
}

impl Default for IgnoreSmallIslands {
    fn default() -> Self {
        IgnoreSmallIslands {
            enabled: false,
            volume_mm3: 0.02,
            height_mm: 0.1,
        }
    }
}

/// Faces leaning more than `angle_deg` from vertical (0° a wall, 90° a flat
/// ceiling) need support, when their region covers at least `min_area_mm2`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverhangRule {
    pub enabled: bool,
    pub angle_deg: f64,
    pub spacing_mm: f64,
    pub size: String,
    pub min_area_mm2: f64,
}

impl Default for OverhangRule {
    fn default() -> Self {
        OverhangRule {
            enabled: true,
            angle_deg: 45.0,
            spacing_mm: 2.0,
            size: String::new(),
            min_area_mm2: 1.0,
        }
    }
}

/// The down-facing surface no other zone covers, in regions of at least
/// `min_area_mm2`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StabilizationRule {
    pub enabled: bool,
    pub spacing_mm: f64,
    pub size: String,
    pub min_area_mm2: f64,
}

impl Default for StabilizationRule {
    fn default() -> Self {
        StabilizationRule {
            enabled: true,
            spacing_mm: 4.0,
            size: String::new(),
            min_area_mm2: 4.0,
        }
    }
}

/// A whole zone preset.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ZoneRules {
    pub sizes: Vec<SupportSize>,
    pub first_layer: Vec<AreaBand>,
    pub ignore_small_islands: IgnoreSmallIslands,
    pub reinforcement: Vec<VolumeBand>,
    pub overhang: OverhangRule,
    pub stabilization: StabilizationRule,
    /// How far a trunk may wander sideways looking for a way down to the
    /// plate, mm.
    pub search_ground_mm: f64,
}

/// The faces down-facing enough to stabilize: steeper than this from
/// horizontal is a wall, which a tip cannot hold.
pub const STABILIZATION_MAX_ANGLE_DEG: f64 = 75.0;

/// The closest two supports of the same zone may stand, as a share of their
/// spacing, and the floor on it (mm).
const DEDUP_SPACING_SHARE: f64 = 0.5;

/// The prefix the sizing table knows zone sizes by, so they never shadow a
/// Support Studio preset id.
pub const ZONE_SIZE_PREFIX: &str = "zone:";

fn finite_in(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl SupportSize {
    /// The size with every number inside what the geometry can draw.
    pub fn clamped(&self) -> Self {
        let d = SupportSize::default();
        let tip = finite_in(self.tip_diameter_mm, 0.05, 5.0, d.tip_diameter_mm);
        SupportSize {
            id: self.id.clone(),
            tip_diameter_mm: tip,
            tip_length_mm: finite_in(self.tip_length_mm, 0.1, 50.0, d.tip_length_mm),
            penetration_mm: finite_in(self.penetration_mm, 0.0, 2.0, 0.0),
            break_point_mm: finite_in(self.break_point_mm, 0.0, 5.0, 0.0),
            trunk_diameter_mm: finite_in(self.trunk_diameter_mm, 0.1, 10.0, d.trunk_diameter_mm),
            base_diameter_mm: finite_in(self.base_diameter_mm, 0.1, 30.0, d.base_diameter_mm),
            base_thickness_mm: finite_in(self.base_thickness_mm, 0.01, 5.0, d.base_thickness_mm),
            join_cone_mm: finite_in(self.join_cone_mm, 0.0, 10.0, d.join_cone_mm),
            base_tip: self.base_tip.map(|b| BaseTip {
                length_mm: finite_in(b.length_mm, 0.1, 50.0, d.tip_length_mm),
                diameter_mm: finite_in(b.diameter_mm, 0.05, 5.0, tip),
            }),
        }
    }

    /// The seven numbers the sizing paths read.
    pub fn band(&self) -> SizingBand {
        SizingBand {
            shaft_diameter_mm: self.trunk_diameter_mm,
            tip_contact_diameter_mm: self.tip_diameter_mm,
            tip_length_mm: self.tip_length_mm,
            tip_penetration_mm: self.penetration_mm,
            root_diameter_mm: self.base_diameter_mm,
            root_disk_height_mm: self.base_thickness_mm,
            // The join cone is not the root's own cone: it is drawn as a 45°
            // cone on the disk or the raft (`Roots::join_cone_mm`).
            root_cone_height_mm: 0.0,
        }
    }
}

impl ZoneRules {
    /// The rules with every number inside its bounds.
    pub fn clamped(&self) -> Self {
        let spacing = |v: f64| finite_in(v, 0.2, 50.0, 2.0);
        ZoneRules {
            sizes: self.sizes.iter().map(SupportSize::clamped).collect(),
            first_layer: self
                .first_layer
                .iter()
                .map(|b| AreaBand {
                    min_area_mm2: finite_in(b.min_area_mm2, 0.0, 1e6, 0.0),
                    spacing_mm: spacing(b.spacing_mm),
                    ..b.clone()
                })
                .collect(),
            ignore_small_islands: IgnoreSmallIslands {
                enabled: self.ignore_small_islands.enabled,
                volume_mm3: finite_in(self.ignore_small_islands.volume_mm3, 0.0, 1e3, 0.02),
                height_mm: finite_in(self.ignore_small_islands.height_mm, 0.0, 100.0, 0.1),
            },
            reinforcement: self
                .reinforcement
                .iter()
                .map(|b| VolumeBand {
                    min_volume_mm3: finite_in(b.min_volume_mm3, 0.0, 1e9, 0.1),
                    height_mm: finite_in(b.height_mm, 0.1, 30.0, 1.5),
                    spacing_mm: spacing(b.spacing_mm),
                    ..b.clone()
                })
                .collect(),
            overhang: OverhangRule {
                angle_deg: finite_in(self.overhang.angle_deg, 15.0, 70.0, 45.0),
                spacing_mm: spacing(self.overhang.spacing_mm),
                min_area_mm2: finite_in(self.overhang.min_area_mm2, 0.0, 1e6, 1.0),
                ..self.overhang.clone()
            },
            stabilization: StabilizationRule {
                spacing_mm: spacing(self.stabilization.spacing_mm),
                min_area_mm2: finite_in(self.stabilization.min_area_mm2, 0.0, 1e6, 4.0),
                ..self.stabilization.clone()
            },
            search_ground_mm: finite_in(self.search_ground_mm, 1.0, 100.0, 30.0),
        }
    }

    /// The index of the size a band names.
    pub fn size_index(&self, id: &str) -> Option<usize> {
        self.sizes.iter().position(|s| s.id == id)
    }

    /// The sizing-table id of size `index`.
    pub fn band_id(&self, index: usize) -> String {
        format!("{ZONE_SIZE_PREFIX}{}", self.sizes[index].id)
    }

    /// The overhang threshold as the scans read it: the self-support angle
    /// from horizontal (faces flatter than it are overhangs).
    pub fn self_support_angle_deg(&self) -> f64 {
        90.0 - self.overhang.angle_deg
    }

    /// The first-layer band an island of `area_mm2` falls in: the enabled
    /// band with the largest minimum it reaches.
    pub fn first_layer_band(&self, area_mm2: f64) -> Option<&AreaBand> {
        self.first_layer
            .iter()
            .filter(|b| {
                b.enabled && area_mm2 >= b.min_area_mm2 && self.size_index(&b.size).is_some()
            })
            .max_by(|a, b| a.min_area_mm2.total_cmp(&b.min_area_mm2))
    }

    /// The reinforcement band an island body of `volume_mm3` falls in.
    pub fn reinforcement_band(&self, volume_mm3: f64) -> Option<&VolumeBand> {
        self.reinforcement
            .iter()
            .filter(|b| {
                b.enabled && volume_mm3 >= b.min_volume_mm3 && self.size_index(&b.size).is_some()
            })
            .max_by(|a, b| a.min_volume_mm3.total_cmp(&b.min_volume_mm3))
    }

    /// The heights above an island's first layer its reinforcement reads the
    /// body's section at, one per enabled band, ascending.
    pub fn section_heights_mm(&self) -> Vec<f64> {
        let mut heights: Vec<f64> = self
            .reinforcement
            .iter()
            .filter(|b| b.enabled)
            .map(|b| b.height_mm)
            .collect();
        heights.sort_by(f64::total_cmp);
        heights.dedup();
        heights
    }

    /// The largest body volume a band tells apart: an island body is
    /// measured up to it, never further.
    pub fn volume_cap_mm3(&self) -> f64 {
        let reinforce = self
            .reinforcement
            .iter()
            .filter(|b| b.enabled)
            .map(|b| b.min_volume_mm3)
            .fold(0.0, f64::max);
        let ignore = if self.ignore_small_islands.enabled {
            self.ignore_small_islands.volume_mm3
        } else {
            0.0
        };
        reinforce.max(ignore)
    }

    /// The size a support placed by hand takes: the overhang size, else the
    /// first island band's, else the first size.
    pub fn default_size_index(&self) -> Option<usize> {
        self.overhang
            .enabled
            .then(|| self.size_index(&self.overhang.size))
            .flatten()
            .or_else(|| {
                self.first_layer
                    .iter()
                    .find_map(|b| self.size_index(&b.size))
            })
            .or((!self.sizes.is_empty()).then_some(0))
    }

    /// How close two supports `spacing_a_mm` and `spacing_b_mm` apart may
    /// stand before the less urgent one is dropped.
    pub fn dedup_radius_mm(spacing_a_mm: f64, spacing_b_mm: f64) -> f64 {
        (spacing_a_mm.max(spacing_b_mm) * DEDUP_SPACING_SHARE).max(0.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> ZoneRules {
        let size = |id: &str, tip: f64| SupportSize {
            id: id.into(),
            tip_diameter_mm: tip,
            ..Default::default()
        };
        let area = |min: f64, size: &str| AreaBand {
            min_area_mm2: min,
            size: size.into(),
            ..Default::default()
        };
        let volume = |min: f64, size: &str| VolumeBand {
            min_volume_mm3: min,
            size: size.into(),
            ..Default::default()
        };
        ZoneRules {
            sizes: vec![size("small", 0.24), size("medium", 0.3), size("large", 0.4)],
            first_layer: vec![
                area(0.0, "small"),
                area(1.05, "medium"),
                area(50.0, "large"),
            ],
            reinforcement: vec![
                volume(0.1, "small"),
                volume(10.0, "medium"),
                volume(95.0, "large"),
            ],
            overhang: OverhangRule {
                angle_deg: 38.0,
                size: "small".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn the_band_is_the_largest_minimum_reached() {
        let r = rules();
        assert_eq!(r.first_layer_band(0.0).unwrap().size, "small");
        assert_eq!(r.first_layer_band(1.05).unwrap().size, "medium");
        assert_eq!(r.first_layer_band(49.9).unwrap().size, "medium");
        assert_eq!(r.first_layer_band(80.0).unwrap().size, "large");
        assert!(r.reinforcement_band(0.05).is_none());
        assert_eq!(r.reinforcement_band(12.0).unwrap().size, "medium");
        assert_eq!(r.reinforcement_band(1000.0).unwrap().size, "large");
        assert_eq!(r.volume_cap_mm3(), 95.0);
    }

    #[test]
    fn a_disabled_band_or_an_unknown_size_is_skipped() {
        let mut r = rules();
        r.first_layer[2].enabled = false;
        assert_eq!(r.first_layer_band(80.0).unwrap().size, "medium");
        r.first_layer[1].size = "nonsense".into();
        assert_eq!(r.first_layer_band(80.0).unwrap().size, "small");
    }

    #[test]
    fn the_overhang_angle_is_read_from_vertical() {
        assert_eq!(rules().self_support_angle_deg(), 52.0);
    }

    #[test]
    fn a_size_is_drawn_as_written() {
        let s = SupportSize {
            id: "x".into(),
            tip_diameter_mm: 0.18,
            ..Default::default()
        };
        let band = s.clamped().band();
        assert_eq!(band.tip_contact_diameter_mm, 0.18);
        assert_eq!(band.shaft_diameter_mm, 1.1);
        assert_eq!(band.root_diameter_mm, 5.0);
        assert_eq!(band.root_disk_height_mm, 0.25);
        assert_eq!(band.root_cone_height_mm, 0.0);
        assert_eq!(band.tip_penetration_mm, 0.0);
    }

    #[test]
    fn rules_round_trip_through_json() {
        let r = rules();
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<ZoneRules>(&text).unwrap(), r);
    }
    /// #406: 0.10 mm means islands that join the rest within 0.10 mm, their
    /// layers included: at 0.05 mm layers, up to two layers.
    #[test]
    fn ignoring_small_islands_counts_their_layers() {
        let ignore = IgnoreSmallIslands {
            enabled: true,
            volume_mm3: 0.02,
            height_mm: 0.10,
        };
        // A body's height, as the scan measures it: layers times 0.05 mm, in
        // single precision.
        let height = |layers: u32| f64::from((f64::from(layers) * 0.05) as f32);
        assert!(ignore.ignores(0.001, height(1)));
        assert!(ignore.ignores(0.001, height(2)));
        assert!(!ignore.ignores(0.001, height(3)));
        assert!(!ignore.ignores(0.02, height(1)), "the volume stays strict");
        let off = IgnoreSmallIslands {
            enabled: false,
            ..ignore
        };
        assert!(!off.ignores(0.001, height(1)));
    }
}

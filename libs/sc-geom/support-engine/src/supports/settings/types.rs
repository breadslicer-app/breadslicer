//! The sizes and rules every support is built with: its tip, shaft and root,
//! the grid, bridges from the model to the model, and the automatic passes.
//! A group missing fields when read takes the defaults for them.

use serde::{Deserialize, Serialize};

use crate::supports::auto_bracing::settings::AutoBracingSettings;
use crate::supports::auto_support::settings::AutoSupportSettings;
use crate::supports::auto_support::zone_rules::BaseTip;
use crate::supports::placement_logic::cone_axis_policy::ConeAngleMode;

/// A tip: the cone from the model to the shaft, and the nib of a disk tip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TipProfile {
    /// Where it touches the model.
    pub contact_diameter_mm: f64,
    /// Where it meets the shaft.
    pub body_diameter_mm: f64,
    pub length_mm: f64,
    /// How far it goes into the model.
    pub penetration_mm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cone_angle_mode: Option<ConeAngleMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_cone_angle_offset_deg: Option<f64>,
    /// A neck this long behind the contact, where the tip snaps off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakpoint_mm: Option<f64>,
    /// A disk tip's nib: its least length, its longest, and the angle
    /// between the surface and the cone past which it grows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_thickness_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_standoff_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standoff_angle_threshold: Option<f64>,
}

impl Default for TipProfile {
    fn default() -> Self {
        TipProfile {
            contact_diameter_mm: 0.3,
            body_diameter_mm: 1.0,
            length_mm: 2.5,
            penetration_mm: 0.1,
            cone_angle_mode: Some(ConeAngleMode::Adaptive),
            adaptive_cone_angle_offset_deg: Some(60.0),
            breakpoint_mm: Some(0.0),
            disk_thickness_mm: Some(0.1),
            max_standoff_mm: Some(1.5),
            standoff_angle_threshold: Some(std::f64::consts::FRAC_PI_4),
        }
    }
}

/// A shaft: its width, and how far from upright it may lean.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShaftProfile {
    pub diameter_mm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_angle_deg: Option<f64>,
}

impl Default for ShaftProfile {
    fn default() -> Self {
        ShaftProfile {
            diameter_mm: 1.0,
            max_angle_deg: Some(80.0),
        }
    }
}

/// A root on the plate: a disk, then a cone up to the shaft.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RootsProfile {
    /// The disk's width.
    pub diameter_mm: f64,
    pub disk_height_mm: f64,
    pub cone_height_mm: f64,
    /// A 45° join cone this tall from the shaft out instead of the cone
    /// (#360).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_cone_mm: Option<f64>,
}

impl Default for RootsProfile {
    fn default() -> Self {
        RootsProfile {
            diameter_mm: 3.0,
            disk_height_mm: 0.5,
            cone_height_mm: 1.5,
            join_cone_mm: None,
        }
    }
}

/// A flare at the foot of a shaft that starts low over the plate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BaseFlareProfile {
    pub enabled: bool,
    pub diameter_mm: f64,
    pub height_mm: f64,
}

impl Default for BaseFlareProfile {
    fn default() -> Self {
        BaseFlareProfile {
            enabled: true,
            diameter_mm: 3.0,
            height_mm: 1.5,
        }
    }
}

/// The grid trunks stand on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GridSettings {
    pub enabled: bool,
    pub spacing_mm: f64,
    /// How steeply a branch may leave its trunk, from horizontal.
    pub min_branch_angle_deg: f64,
    /// The step a search for a joint on a trunk takes.
    pub attach_search_step_mm: f64,
    /// How steeply a trunk routed off its node may run, from horizontal.
    pub min_routed_trunk_angle_deg: f64,
    /// Pillars on the nodes carry what hangs from them: the highest join
    /// first, a wider search, and neighbours for what no node sees (#464).
    #[serde(skip)]
    pub pillars: bool,
}

impl Default for GridSettings {
    fn default() -> Self {
        GridSettings {
            enabled: false,
            spacing_mm: 4.0,
            min_branch_angle_deg: 60.0,
            attach_search_step_mm: 2.0,
            min_routed_trunk_angle_deg: 60.0,
            pillars: false,
        }
    }
}

/// Bridges from the model to the model.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MeshToMeshSettings {
    /// Spans past this are sticks, shorter ones twigs.
    pub stick_vs_twig_cutoff_mm: f64,
    /// The tip a stick ends in on the model below; none ends it in the
    /// upper tip (#360).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_tip: Option<BaseTip>,
}

impl Default for MeshToMeshSettings {
    fn default() -> Self {
        MeshToMeshSettings {
            stick_vs_twig_cutoff_mm: 5.0,
            base_tip: None,
        }
    }
}

/// Everything supports are built with.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SupportSettings {
    pub tip: TipProfile,
    pub shaft: ShaftProfile,
    pub roots: RootsProfile,
    pub base_flare: BaseFlareProfile,
    pub grid: GridSettings,
    pub mesh_to_mesh: MeshToMeshSettings,
    pub auto_bracing: AutoBracingSettings,
    pub auto_support: AutoSupportSettings,
}

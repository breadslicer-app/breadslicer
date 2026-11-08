//! DragonFruit `src/supports/Settings/types.ts`.
//!
//! Support Settings Types
//!
//! Defines all configuration interfaces for support geometry.
//! This is the single source of truth for settings shapes.
//!
//! Every group deserializes with DragonFruit's defaults for absent fields,
//! which is what `mergeSettingsWithDefaults` does to a partial payload.

use serde::{Deserialize, Serialize};

use super::defaults::*;
use crate::supports::auto_bracing::settings::AutoBracingSettings;
use crate::supports::auto_support::settings::AutoSupportSettings;
use crate::supports::auto_support::zone_rules::BaseTip;
use crate::supports::placement_logic::cone_axis_policy::ConeAngleMode;
use crate::supports::support_primitives::contact_cone::types::TipPrimitive;

/// The only tip shape: `'cone'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TipShape {
    #[default]
    Cone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BodyShape {
    #[default]
    Cylinder,
    Cube,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingAlgorithm {
    Astar,
    #[default]
    Potential,
}

// --- Profile Types ---

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TipProfile {
    pub shape: TipShape,
    /// Phase 1 Contact Primitives
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<TipPrimitive>,
    /// Small end touching model
    pub contact_diameter_mm: f64,
    /// Larger end (socket side)
    pub body_diameter_mm: f64,
    /// Total cone length
    pub length_mm: f64,
    /// Embed depth into model
    pub penetration_mm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cone_angle_mode: Option<ConeAngleMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_cone_angle_offset_deg: Option<f64>,
    /// Overall cone profile (derived)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cone_angle_deg: Option<f64>,
    /// Optional internal breakpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakpoint_mm: Option<f64>,

    // Contact Disk Props
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_thickness_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_standoff_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standoff_angle_threshold: Option<f64>,

    // Contact Sphere Props
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sphere_radius_ratio: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShaftProfile {
    pub shape: BodyShape,
    pub diameter_mm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_diameter_mm: Option<f64>,
    pub is_straight: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_angle_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_algorithm: Option<RoutingAlgorithm>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RootsProfile {
    pub shape: BodyShape,
    /// Base disk diameter
    pub diameter_mm: f64,
    /// Flat disk height
    pub disk_height_mm: f64,
    /// Transition cone height
    pub cone_height_mm: f64,
    /// Where it meets the shaft
    pub neck_diameter_mm: f64,
    /// 0..1 blend factor
    pub neck_blend: f64,
    /// SoulCrafted (#360): a 45° join cone of this height from the shaft
    /// out, on the disk or on the raft (see `Roots::join_cone_mm`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_cone_mm: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BaseFlareProfile {
    pub enabled: bool,
    pub diameter_mm: f64,
    pub height_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JointProfile {
    pub ball_diameter_mm: f64,
    pub max_rotation_deg: f64,
    pub max_slide_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GridSettings {
    pub enabled: bool,
    pub spacing_mm: f64,
    pub min_branch_angle_deg: f64,
    pub attach_search_step_mm: f64,
    pub min_routed_trunk_angle_deg: f64,
    /// SoulCrafted (#464): pillars on the nodes carry what hangs from them:
    /// highest join first, a wider search, and neighbours for what no node
    /// sees. Off is DragonFruit's grid.
    #[serde(skip)]
    pub pillars: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MeshToMeshSettings {
    pub stick_vs_twig_cutoff_mm: f64,
    /// SoulCrafted (#360): the tip a stick ends in at its lower end, on the
    /// model below; `None` ends it in the upper tip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_tip: Option<BaseTip>,
    /// SoulCrafted (#445): a cavity bridge the first surface it finds turns
    /// down goes on to the other surfaces around the contact.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub every_landing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DevToolsSettings {
    pub routing_algorithm: RoutingAlgorithm,
    pub field_deterministic: bool,
    pub clearance_mm: f64,
    pub margin_mm: f64,
    pub repulsion_strength: f64,
    pub step_mm: f64,
    pub max_lateral_mm: f64,
    pub tangent_weight: f64,
    pub max_nearest_node_search_rings: f64,
    pub cone_stretch_linear_weight: f64,
    pub cone_stretch_quadratic_weight: f64,
    pub cone_angle_weight: f64,
    pub max_cone_stretch_ratio: f64,
    pub max_vertical_attachment_distance_mm: f64,
    pub max_horizontal_attachment_distance_mm: f64,
    pub min_horizontal_leaf_angle_deg: f64,
    pub max_leaf_stretch_factor: f64,
    pub max_cone_angle_dev_deg: f64,
    pub vertical_knot_spacing_mm: f64,
    pub max_branches_per_trunk: f64,
}

// --- Main Settings Interface ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SupportSettings {
    pub tip: TipProfile,
    pub shaft: ShaftProfile,
    pub roots: RootsProfile,
    pub base_flare: BaseFlareProfile,
    pub joint: JointProfile,
    pub grid: GridSettings,
    pub mesh_to_mesh: MeshToMeshSettings,
    pub auto_bracing: AutoBracingSettings,
    pub auto_support: AutoSupportSettings,
    pub dev_tools_enabled: bool,
    pub dev_tools: DevToolsSettings,
    /// Debug: only render contact disk/cone + line vector for shaft (like J×2 pathfinding debug).
    pub debug_simple_support_render: bool,
    /// View: only the contact discs stay solid meshes, every other member is a
    /// line vector. A navigation aid for a dense forest, and not part of a
    /// preset, so switching presets cannot flip the view.
    pub navigation_discs_only: bool,
}

// --- Default Factory ---

pub fn create_default_settings() -> SupportSettings {
    SupportSettings {
        tip: TipProfile::default(),
        shaft: ShaftProfile::default(),
        roots: RootsProfile::default(),
        base_flare: BaseFlareProfile::default(),
        joint: JointProfile::default(),
        grid: GridSettings::default(),
        mesh_to_mesh: MeshToMeshSettings::default(),
        auto_bracing: AutoBracingSettings::default(),
        auto_support: AutoSupportSettings::default(),
        dev_tools_enabled: false,
        dev_tools: DevToolsSettings::default(),
        debug_simple_support_render: false,
        navigation_discs_only: false,
    }
}

impl Default for SupportSettings {
    fn default() -> Self {
        create_default_settings()
    }
}

impl Default for TipProfile {
    fn default() -> Self {
        TipProfile {
            shape: TipShape::Cone,
            kind: Some(TipPrimitive::Disk),
            contact_diameter_mm: DEFAULT_TIP_CONTACT_DIAMETER_MM,
            body_diameter_mm: DEFAULT_TIP_BODY_DIAMETER_MM,
            length_mm: DEFAULT_TIP_LENGTH_MM,
            penetration_mm: DEFAULT_TIP_PENETRATION_MM,
            cone_angle_mode: Some(ConeAngleMode::Adaptive),
            adaptive_cone_angle_offset_deg: Some(60.0),
            cone_angle_deg: Some(DEFAULT_TIP_CONE_ANGLE_DEG),
            breakpoint_mm: Some(DEFAULT_TIP_BREAKPOINT_MM),
            // Disk Defaults
            disk_thickness_mm: Some(0.1),
            max_standoff_mm: Some(1.5),
            standoff_angle_threshold: Some(std::f64::consts::PI / 4.0),
            sphere_radius_ratio: None,
        }
    }
}

impl Default for ShaftProfile {
    fn default() -> Self {
        ShaftProfile {
            shape: BodyShape::Cylinder,
            diameter_mm: DEFAULT_SHAFT_DIAMETER_MM,
            secondary_diameter_mm: Some(DEFAULT_SHAFT_DIAMETER_MM),
            is_straight: true,
            max_angle_deg: Some(DEFAULT_SHAFT_MAX_ANGLE_DEG),
            routing_algorithm: Some(RoutingAlgorithm::Potential),
        }
    }
}

impl Default for RootsProfile {
    fn default() -> Self {
        RootsProfile {
            shape: BodyShape::Cylinder,
            diameter_mm: DEFAULT_ROOTS_DIAMETER_MM,
            disk_height_mm: DEFAULT_ROOTS_DISK_HEIGHT_MM,
            cone_height_mm: DEFAULT_ROOTS_CONE_HEIGHT_MM,
            neck_diameter_mm: DEFAULT_ROOTS_NECK_DIAMETER_MM,
            neck_blend: DEFAULT_ROOTS_NECK_BLEND,
            join_cone_mm: None,
        }
    }
}

impl Default for BaseFlareProfile {
    fn default() -> Self {
        BaseFlareProfile {
            enabled: DEFAULT_BASE_FLARE_ENABLED,
            diameter_mm: DEFAULT_BASE_FLARE_DIAMETER_MM,
            height_mm: DEFAULT_BASE_FLARE_HEIGHT_MM,
        }
    }
}

impl Default for JointProfile {
    fn default() -> Self {
        JointProfile {
            ball_diameter_mm: DEFAULT_JOINT_BALL_DIAMETER_MM,
            max_rotation_deg: DEFAULT_JOINT_MAX_ROTATION_DEG,
            max_slide_mm: DEFAULT_JOINT_MAX_SLIDE_MM,
        }
    }
}

impl Default for GridSettings {
    fn default() -> Self {
        GridSettings {
            enabled: DEFAULT_GRID_ENABLED,
            spacing_mm: DEFAULT_GRID_SPACING_MM,
            pillars: false,
            min_branch_angle_deg: DEFAULT_GRID_MIN_BRANCH_ANGLE_DEG,
            attach_search_step_mm: DEFAULT_GRID_ATTACH_SEARCH_STEP_MM,
            min_routed_trunk_angle_deg: DEFAULT_GRID_MIN_ROUTED_TRUNK_ANGLE_DEG,
        }
    }
}

impl Default for MeshToMeshSettings {
    fn default() -> Self {
        MeshToMeshSettings {
            stick_vs_twig_cutoff_mm: DEFAULT_MESH_TO_MESH_STICK_VS_TWIG_CUTOFF_MM,
            base_tip: None,
            every_landing: false,
        }
    }
}

impl Default for DevToolsSettings {
    fn default() -> Self {
        DevToolsSettings {
            routing_algorithm: RoutingAlgorithm::Potential,
            field_deterministic: true,
            clearance_mm: 1.5,
            margin_mm: 2.5,
            repulsion_strength: 8.0,
            step_mm: 1.0,
            max_lateral_mm: 30.0,
            tangent_weight: 0.5,
            max_nearest_node_search_rings: 12.0,
            cone_stretch_linear_weight: 7.5,
            cone_stretch_quadratic_weight: 4.0,
            cone_angle_weight: 0.04,
            max_cone_stretch_ratio: 0.52,
            max_vertical_attachment_distance_mm: 40.0,
            max_horizontal_attachment_distance_mm: 15.0,
            min_horizontal_leaf_angle_deg: 30.0,
            max_leaf_stretch_factor: 2.0,
            max_cone_angle_dev_deg: 30.0,
            vertical_knot_spacing_mm: 3.0,
            max_branches_per_trunk: 3.0,
        }
    }
}

// --- Preset Types ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportPreset {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub is_built_in: bool,
    /// Pinned slot (1-4) for quick-access hotkeys, or undefined if unpinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_slot: Option<f64>,
    pub settings: SupportSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetCollection {
    pub by_id: indexmap::IndexMap<String, SupportPreset>,
    pub all_ids: Vec<String>,
    pub active_preset_id: Option<String>,
}

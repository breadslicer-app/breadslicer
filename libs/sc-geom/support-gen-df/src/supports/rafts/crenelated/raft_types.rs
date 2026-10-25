//! DragonFruit `src/supports/Rafts/Crenelated/RaftTypes.ts` and
//! `RaftDefaults.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RaftBottomMode {
    Off,
    Solid,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RaftSettings {
    pub bottom_mode: RaftBottomMode,
    pub wall_enabled: bool,
    /// mm
    pub thickness: f64,
    /// degrees (45-90)
    pub chamfer_angle: f64,
    /// mm
    pub wall_height: f64,
    /// mm
    pub wall_thickness: f64,
    /// mm
    pub crenulation_gap_width: f64,
    /// mm
    pub crenulation_spacing: f64,
    /// mm
    pub line_width_mm: f64,
    /// mm
    pub line_height_mm: f64,
    /// Show the combined model+raft footprint border.
    pub show_footprint_border: bool,
    /// mm beyond the raft/model edge.
    pub footprint_border_margin: f64,
}

/// `DEFAULT_RAFT_SETTINGS`.
pub const DEFAULT_RAFT_SETTINGS: RaftSettings = RaftSettings {
    bottom_mode: RaftBottomMode::Solid,
    wall_enabled: true,
    thickness: 0.5,
    chamfer_angle: 45.0,
    wall_height: 0.35,
    wall_thickness: 0.5,
    crenulation_gap_width: 1.5,
    crenulation_spacing: 5.0,
    line_width_mm: 1.5,
    line_height_mm: 0.6,
    show_footprint_border: true,
    footprint_border_margin: 0.5,
};

impl Default for RaftSettings {
    fn default() -> Self {
        DEFAULT_RAFT_SETTINGS
    }
}

/// A support base footprint circle (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SupportBaseCircle {
    pub x: f64,
    pub y: f64,
    /// Radius (half the base diameter).
    pub r: f64,
}

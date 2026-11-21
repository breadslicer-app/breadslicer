//! What a raft is made of: its kind, its sizes, and the circles of the
//! support roots it is drawn around.

use serde::{Deserialize, Serialize};

/// What the supports stand on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RaftBottomMode {
    /// Each root's own foot on the plate.
    Off,
    /// One plate under the roots.
    Solid,
    /// Beams joining the roots.
    Line,
}

/// A raft's shape. Lengths in millimetres, the chamfer in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RaftSettings {
    pub bottom_mode: RaftBottomMode,
    /// A wall around a solid raft's edge (a line raft has none).
    pub wall_enabled: bool,
    /// A solid raft's plate.
    pub thickness: f64,
    /// How steep the raft's sides are, 45° to 90° (upright).
    pub chamfer_angle: f64,
    pub wall_height: f64,
    pub wall_thickness: f64,
    /// The gaps in the wall, and how far apart they are (0: no gaps).
    pub crenulation_gap_width: f64,
    pub crenulation_spacing: f64,
    /// A line raft's beams.
    pub line_width_mm: f64,
    pub line_height_mm: f64,
    /// Whether the view outlines what the models and rafts take on the plate,
    /// and how far past them (at most 0.05 mm).
    pub show_footprint_border: bool,
    pub footprint_border_margin: f64,
    /// A solid raft's holes between the supports, this wide (#138); 0 for
    /// none.
    #[serde(default)]
    pub perforation_mm: f64,
    /// A pad under each support instead of one plate (#138).
    #[serde(default)]
    pub pads: bool,
    /// Tabs off a solid raft's two ends, to lever it off (#99).
    #[serde(default)]
    pub peel_tabs: bool,
}

/// The raft a set gets unless told otherwise.
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
    perforation_mm: 0.0,
    pads: false,
    peel_tabs: false,
};

impl Default for RaftSettings {
    fn default() -> Self {
        DEFAULT_RAFT_SETTINGS
    }
}

impl RaftSettings {
    /// These settings as a raft can take them: a wall only on a solid raft,
    /// and the outline's margin between 0 and 0.05 mm.
    pub fn settled(self) -> Self {
        RaftSettings {
            wall_enabled: self.wall_enabled && self.bottom_mode == RaftBottomMode::Solid,
            footprint_border_margin: self.footprint_border_margin.clamp(0.0, 0.05),
            ..self
        }
    }
}

/// Where a support root stands on the plate, and its radius (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SupportBaseCircle {
    pub x: f64,
    pub y: f64,
    pub r: f64,
}

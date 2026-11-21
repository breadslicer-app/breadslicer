//! DragonFruit `src/supports/Settings/defaults.ts`.
//!
//! Default Support Settings
//!
//! Edit these values to change the initial settings for new users
//! or when resetting to defaults.

// --- Tip (Contact Cone) ---
pub const DEFAULT_TIP_CONTACT_DIAMETER_MM: f64 = 0.3;
pub const DEFAULT_TIP_BODY_DIAMETER_MM: f64 = 1.0;
pub const DEFAULT_TIP_LENGTH_MM: f64 = 2.5;
pub const DEFAULT_TIP_PENETRATION_MM: f64 = 0.1;
pub const DEFAULT_TIP_CONE_ANGLE_DEG: f64 = 100.0;
pub const DEFAULT_TIP_BREAKPOINT_MM: f64 = 0.0;

// --- Shaft ---
pub const DEFAULT_SHAFT_DIAMETER_MM: f64 = 1.0;
pub const DEFAULT_SHAFT_MAX_ANGLE_DEG: f64 = 80.0;

// --- Roots (Base) ---
pub const DEFAULT_ROOTS_DIAMETER_MM: f64 = 3.0;
pub const DEFAULT_ROOTS_DISK_HEIGHT_MM: f64 = 0.5;
pub const DEFAULT_ROOTS_CONE_HEIGHT_MM: f64 = 1.5;
pub const DEFAULT_ROOTS_NECK_DIAMETER_MM: f64 = 1.0;
pub const DEFAULT_ROOTS_NECK_BLEND: f64 = 0.7;

// --- Base Flare ---
pub const DEFAULT_BASE_FLARE_ENABLED: bool = true;
pub const DEFAULT_BASE_FLARE_DIAMETER_MM: f64 = 3.0;
pub const DEFAULT_BASE_FLARE_HEIGHT_MM: f64 = 1.5;

// --- Joint ---
pub const DEFAULT_JOINT_BALL_DIAMETER_MM: f64 = 1.5;
pub const DEFAULT_JOINT_MAX_ROTATION_DEG: f64 = 45.0;
pub const DEFAULT_JOINT_MAX_SLIDE_MM: f64 = 5.0;

// --- Grid ---
pub const DEFAULT_GRID_ENABLED: bool = false;
pub const DEFAULT_GRID_SPACING_MM: f64 = 4.0;

pub const DEFAULT_GRID_MIN_BRANCH_ANGLE_DEG: f64 = 60.0;
pub const DEFAULT_GRID_ATTACH_SEARCH_STEP_MM: f64 = 2.0;
pub const DEFAULT_GRID_MIN_ROUTED_TRUNK_ANGLE_DEG: f64 = 60.0;

pub const DEFAULT_MESH_TO_MESH_STICK_VS_TWIG_CUTOFF_MM: f64 = 5.0;

// --- Profile field limits ---

/// A sane `[min, max]` range for one profile field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldLimit {
    pub min: f64,
    pub max: f64,
}

/// Sane ranges for the General tab's profile fields, in one place: the settings
/// store clamps every write through them (UI, preset, import, plugin) so a
/// negative or absurd value can never reach the geometry, and the inputs take
/// their `min`/`max` from the same table.
///
/// Lower bounds are the smallest value that still means something: 0 for a height
/// that may legitimately be "no feature", a hair above 0 for a diameter, because a
/// zero-radius disk/cone has no usable normal. Upper bounds are generous — they
/// catch a typo (a stray digit), not model a printer.
pub mod support_profile_limits {
    pub mod tip {
        use super::super::FieldLimit;
        pub const CONTACT_DIAMETER_MM: FieldLimit = FieldLimit {
            min: 0.05,
            max: 5.0,
        };
        pub const LENGTH_MM: FieldLimit = FieldLimit {
            min: 0.1,
            max: 50.0,
        };
        pub const ADAPTIVE_CONE_ANGLE_OFFSET_DEG: FieldLimit = FieldLimit {
            min: 0.0,
            max: 90.0,
        };
    }

    pub mod shaft {
        use super::super::FieldLimit;
        pub const DIAMETER_MM: FieldLimit = FieldLimit {
            min: 0.05,
            max: 20.0,
        };
    }

    pub mod roots {
        use super::super::FieldLimit;
        pub const DIAMETER_MM: FieldLimit = FieldLimit {
            min: 0.05,
            max: 50.0,
        };
        pub const DISK_HEIGHT_MM: FieldLimit = FieldLimit {
            min: 0.0,
            max: 20.0,
        };
        pub const CONE_HEIGHT_MM: FieldLimit = FieldLimit {
            min: 0.0,
            max: 50.0,
        };
    }
}

//! DragonFruit `src/supports/PlacementLogic/ShaftAngleConstraint.ts`.
//!
//! Angle limit from vertical (0 degrees = vertical, 90 degrees = horizontal).
//! We want to prevent supports from being too flat (horizontal) or pointing upwards (negative).
//! Max 80 degrees means the support must be at least 10 degrees away from horizontal.
//! Note: This default is now 80, but callers should usually provide the user setting.


/// DragonFruit's defaults: `maxAngleDeg = 80`, `referenceAxis = { 0, 0, 1 }`.
pub const DEFAULT_MAX_SHAFT_ANGLE_DEG: f64 = 80.0;

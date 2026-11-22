//! How wide a brace is at each end.

use serde::Serialize;

use support_geom::geom::math_utils::{js_max, js_min};
use support_model::constants::JOINT_DIAMETER_OFFSET_MM;
use support_model::types::BraceProfile;

/// The narrowest a brace end gets, in mm.
const MIN_DIAMETER_MM: f64 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BraceSpanDiameters {
    pub start_diameter_mm: f64,
    pub end_diameter_mm: f64,
    pub is_tapered: bool,
}

/// The brace's width where it meets each knot: the knot's width less the
/// joint offset, never wider than the brace's own profile. Ends of different
/// widths make it taper. Drawing, picking and export all size it from here.
///
/// `profile` is the brace's (older braces have none, and are 1 mm); a knot
/// diameter is `None` for a missing knot or one with no width, and such an
/// end takes the profile's width.
pub fn brace_span_diameters(
    profile: Option<BraceProfile>,
    start_knot_diameter: Option<f64>,
    end_knot_diameter: Option<f64>,
) -> BraceSpanDiameters {
    let widest = js_max(MIN_DIAMETER_MM, profile.map_or(1.0, |p| p.diameter));
    let end_width = |knot: Option<f64>| {
        let knot = knot.unwrap_or(widest + JOINT_DIAMETER_OFFSET_MM);
        js_min(
            widest,
            js_max(MIN_DIAMETER_MM, knot - JOINT_DIAMETER_OFFSET_MM),
        )
    };
    let start_diameter_mm = end_width(start_knot_diameter);
    let end_diameter_mm = end_width(end_knot_diameter);
    BraceSpanDiameters {
        start_diameter_mm,
        end_diameter_mm,
        is_tapered: (start_diameter_mm - end_diameter_mm).abs() > 1e-4,
    }
}

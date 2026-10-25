//! DragonFruit `src/supports/SupportTypes/Brace/braceSpanDiameters.ts`.

use serde::Serialize;

use crate::supports::constants::JOINT_DIAMETER_OFFSET_MM;
use crate::supports::types::BraceProfile;
use crate::three::math_utils::{js_max, js_min};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BraceSpanDiameters {
    pub start_diameter_mm: f64,
    pub end_diameter_mm: f64,
    pub is_tapered: bool,
}

/// How wide a brace is at each knot it spans: the knot's width less the joint
/// offset, capped by the brace's profile. Differing ends are what makes it
/// taper. The renderer, the proxy layer and the exporter all draw the same brace.
///
/// `profile` is `brace.profile` (absent on a legacy brace); the knots are the
/// spanned knots' diameters, `None` for a missing knot or one without a width.
pub fn brace_span_diameters(
    profile: Option<BraceProfile>,
    start_knot_diameter: Option<f64>,
    end_knot_diameter: Option<f64>,
) -> BraceSpanDiameters {
    let profile_diameter = js_max(0.001, profile.map_or(1.0, |p| p.diameter));
    let at_knot = |diameter: Option<f64>| {
        js_min(
            profile_diameter,
            js_max(
                0.001,
                diameter.unwrap_or(profile_diameter + JOINT_DIAMETER_OFFSET_MM)
                    - JOINT_DIAMETER_OFFSET_MM,
            ),
        )
    };

    let start_diameter_mm = at_knot(start_knot_diameter);
    let end_diameter_mm = at_knot(end_knot_diameter);
    BraceSpanDiameters {
        start_diameter_mm,
        end_diameter_mm,
        is_tapered: (start_diameter_mm - end_diameter_mm).abs() > 1e-4,
    }
}

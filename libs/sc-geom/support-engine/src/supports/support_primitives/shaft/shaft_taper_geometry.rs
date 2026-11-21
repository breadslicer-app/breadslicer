//! DragonFruit `src/supports/SupportPrimitives/Shaft/shaftTaperGeometry.ts`.

use crate::three::math_utils::{clamp, lerp};

/// The width of a tapering shaft at each point of the polyline drawing it, so a
/// curve's pieces share the taper rather than each carrying all of it.
pub fn taper_width_along(
    point_count: usize,
    start_diameter_mm: f64,
    end_diameter_mm: Option<f64>,
) -> impl Fn(usize) -> f64 {
    let spans = point_count.saturating_sub(1).max(1) as f64;
    let end = end_diameter_mm.unwrap_or(start_diameter_mm);
    let uniform = (end - start_diameter_mm).abs() < 1e-6;
    move |point_index: usize| {
        if uniform {
            return start_diameter_mm;
        }
        lerp(
            start_diameter_mm,
            end,
            clamp(point_index as f64 / spans, 0.0, 1.0),
        )
    }
}

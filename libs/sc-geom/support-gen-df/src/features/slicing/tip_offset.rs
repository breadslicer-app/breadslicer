//! DragonFruit `supports/rendering/calculateTipOffset.ts` for the default
//! material anti-aliasing settings (`DEFAULT_MATERIAL_ANTI_ALIASING_SETTINGS`,
//! no custom override): how far the slice export sinks contact tips into the
//! model, so the anti-aliasing blur does not leave them short of it.
//!
//! Without an override, `resolveEffectiveAaSettings` takes the physical
//! auto configuration of `autoAaPhysics.ts` for the `balanced` preset, of
//! which the offset reads only the Z blur radius: `(2 r + 1)` layers. The
//! custom-override branches read DragonFruit material fields SoulCrafted's
//! profiles do not have, and are not ported.

use crate::three::math_utils::js_to_fixed_number;

/// `MIN_ASPECT_RATIO_FOR_3DAA.balanced`.
const MIN_ASPECT_RATIO_FOR_3DAA: f64 = 0.35;

/// `calculateTipOffset(DEFAULT_MATERIAL_ANTI_ALIASING_SETTINGS, layer,
/// pitch_x, pitch_y)`, in millimetres.
pub fn default_tip_offset_mm(layer_height_mm: f64, pitch_x_mm: f64, pitch_y_mm: f64) -> f64 {
    let layer = layer_height_mm.max(0.001);
    // resolveEffectiveAaSettings clamps at 1e-4, computePhysicalAaConfig again.
    let (px, py) = (pitch_x_mm.max(0.0001), pitch_y_mm.max(0.0001));
    let pitch = (px.min(py) * px.max(py)).sqrt();
    let aspect = layer / pitch;
    // resolveBackendMode, then computeZBlurRadiusLayers (no preset boost).
    let z_blur_radius_layers = if aspect >= MIN_ASPECT_RATIO_FOR_3DAA {
        let base: f64 = if aspect >= 1.6 { 2.0 } else { 1.0 };
        base.clamp(1.0, 4.0)
    } else {
        0.0
    };
    let auto = js_to_fixed_number((2.0 * z_blur_radius_layers + 1.0) * layer, 3);
    js_to_fixed_number(auto, 3)
}

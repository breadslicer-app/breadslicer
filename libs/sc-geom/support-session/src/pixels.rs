//! Tips the printer's screen can draw (#724).
//!
//! A tip touches the model with a disc of its contact diameter, and the
//! screen lights that disc pixel by pixel. Measured on the pixel grid
//! (`sl1cer/tests/thin_tips.rs`): a disc under 2.5 pixels across is never
//! fully lit with antialiasing on, wherever it falls, and under 1.5 pixels
//! it can vanish with antialiasing off; from 2.83 pixels across it always
//! lights a full pixel. A tip thinner than the screen draws looks fine in
//! the view and lets go in the print, so every tip a set is made with is
//! at least [`MIN_TIP_PIXELS`] of the printer's pixels across.

use bread_support_model::auto_support::zone_rules::SupportSize;
use bread_support_pipeline::PipelineSettings;

/// The narrowest tip, in pixels of the printer's screen: past the 2.83 a
/// disc needs to light a full pixel wherever it falls, with some room.
pub const MIN_TIP_PIXELS: f64 = 3.0;

/// The narrowest tip a screen of `pitch` (mm per pixel in x and y) draws.
pub fn narrowest_tip_mm(pitch: [f64; 2]) -> f64 {
    MIN_TIP_PIXELS * pitch[0].max(pitch[1])
}

/// Widens every tip of `settings` to `narrowest` mm at least, and the shaft
/// it ends in with it when the shaft would be thinner than the tip.
pub(crate) fn widen_tips(settings: &mut PipelineSettings, narrowest: f64) {
    if !(narrowest.is_finite() && narrowest > 0.0) {
        return;
    }
    let supports = &mut settings.supports;
    let tip = &mut supports.tip;
    tip.contact_diameter_mm = tip.contact_diameter_mm.max(narrowest);
    tip.body_diameter_mm = tip.body_diameter_mm.max(tip.contact_diameter_mm);
    supports.shaft.diameter_mm = supports.shaft.diameter_mm.max(tip.contact_diameter_mm);
    for size in &mut supports.auto_support.zone_rules.sizes {
        widen_size(size, narrowest);
    }
}

/// Widens one size's tips to `narrowest` mm at least.
pub(crate) fn widen_size(size: &mut SupportSize, narrowest: f64) {
    size.tip_diameter_mm = size.tip_diameter_mm.max(narrowest);
    size.trunk_diameter_mm = size.trunk_diameter_mm.max(size.tip_diameter_mm);
    if let Some(base) = &mut size.base_tip {
        base.diameter_mm = base.diameter_mm.max(narrowest);
    }
}

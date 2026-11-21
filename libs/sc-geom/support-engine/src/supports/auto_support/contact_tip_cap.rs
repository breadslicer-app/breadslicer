//! DragonFruit `src/supports/autoSupport/contactTipCap.ts`.
//!
//! Tip contact cap by local free width. A contact tip is a disc sitting on the
//! model and it must FIT the feature it lands on. The free width is the width
//! of the free span at the contact, measured in the plane the tip lands on:
//! the probe steps out along the tangent plane `CONTACT_WIDTH_PROBE_MM` in each
//! of the four tangent directions and reads how far it walked past the
//! feature's silhouette (`reach - d`); the sum over the two signs of the
//! narrower axis is the span the disc has to fit inside. The SDF is the oracle
//! the rest of placement routes against, so this queries the same field.

use super::constants::CONTACT_WIDTH_PROBE_MM;
use super::parameter_sizing::SizingBands;
use super::types::CandidatePoint;
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::settings::types::SupportSettings;
use crate::supports::types::Vec3;
use crate::three::math_utils::{js_max, js_min};
use crate::utils::math::round;

/// Local free width (mm) at a contact, from the shared distance field.
pub fn local_free_width_mm(sdf: &dyn CollisionField, x: f64, y: f64, z: f64, normal: Vec3) -> f64 {
    let mut n = normal;
    if n.length_sq() < 1e-12 {
        n = Vec3::new(0.0, 0.0, -1.0);
    }
    let n = n.normalize();

    // Any reference not parallel to the normal gives a tangent basis. The probe
    // runs in the plane the tip's disc lies in, not in world XY.
    let reference = if n.z.abs() > 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 0.0, 1.0)
    };
    let tangent1 = Vec3::cross_vectors(n, reference).normalize();
    let tangent2 = Vec3::cross_vectors(n, tangent1).normalize();

    let reach = CONTACT_WIDTH_PROBE_MM;
    let mut min_span_mm = 2.0 * reach;
    for t in [tangent1, tangent2] {
        let mut span_mm = 0.0;
        for sign in [1.0, -1.0] {
            let d = sdf.exact_signed_distance_at(Vec3::new(
                x + t.x * reach * sign,
                y + t.y * reach * sign,
                z + t.z * reach * sign,
            ));
            // d ≤ 0: the probe is still on (or inside) the feature, so that
            // side reaches at least the full probe. d > 0: the feature's edge
            // sits `reach - d` from the contact on this side, never negative.
            span_mm += if d.is_finite() && d > 0.0 {
                js_max(0.0, reach - d)
            } else {
                reach
            };
        }
        if span_mm < min_span_mm {
            min_span_mm = span_mm;
        }
    }
    js_max(0.0, min_span_mm)
}

/// Clamp a candidate's tip contact by the local free width. Shrinks only, and
/// floored at the small-island tip. A width of 0 is the probe finding no
/// clearance in either direction (a ridge, crease or rim read edge-on), which
/// is inconclusive rather than narrow, so it does not cap.
pub fn apply_contact_tip_cap(
    candidate: &mut CandidatePoint,
    sdf: &dyn CollisionField,
    settings: &SupportSettings,
    bands: &SizingBands,
) {
    let ceiling_mm = candidate
        .tip_diameter_mm
        .unwrap_or_else(|| bands.active(settings).tip_contact_diameter_mm);
    let floor_mm = bands.small_island_tip_diameter_mm();
    if ceiling_mm <= floor_mm {
        return;
    }
    let free_width_mm = local_free_width_mm(
        sdf,
        candidate.tip_pos.x,
        candidate.tip_pos.y,
        candidate.tip_pos.z,
        candidate.tip_normal,
    );
    if free_width_mm <= 0.0 {
        return;
    }
    let margin_scale = settings.auto_support.tip_contact_margin_scale;
    let target_mm = js_max(js_min(ceiling_mm, free_width_mm * margin_scale), floor_mm);
    if target_mm >= ceiling_mm {
        return;
    }
    candidate.tip_diameter_mm = Some(round(target_mm, 3));
}

/// Cap every candidate against the mesh's distance field. A caller without a
/// field keeps band sizing.
pub fn apply_contact_tip_caps(
    candidates: &mut [CandidatePoint],
    sdf: Option<&dyn CollisionField>,
    settings: &SupportSettings,
    bands: &SizingBands,
) {
    let Some(sdf) = sdf else { return };
    for candidate in candidates {
        apply_contact_tip_cap(candidate, sdf, settings, bands);
    }
}

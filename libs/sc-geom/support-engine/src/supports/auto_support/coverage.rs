//! How much of an overhang region the tips placed cover.

use std::collections::HashMap;

use super::constants::influence_radius_mm;
use crate::geom::math_utils::js_max;
use crate::supports::types::Vec3;
use crate::volume_analysis::islands::types::DetectedIsland;
use crate::volume_analysis::islands::voxel_footprint::cell_key_bits;

/// The radius a tip covers on its own surface, mm.
pub const TIP_COVERAGE_RADIUS_MM: f64 = 3.0;

/// The cover radius over a region of `area_mm2`: a large flat region peels
/// harder, so its tips cover less (down to half of `base_radius_mm`).
pub fn coverage_radius_for_area(area_mm2: f64, base_radius_mm: f64) -> f64 {
    if !(area_mm2 > 0.0) {
        return base_radius_mm;
    }
    base_radius_mm * js_max(0.5, (8.0 / js_max(area_mm2, 8.0)).sqrt())
}

/// The square of the radius a tip at `tip_z` covers at height `z`.
fn reach2(z: f64, tip_z: f64, radius_mm: f64) -> f64 {
    let r = radius_mm + js_max(0.0, influence_radius_mm(z - tip_z) - TIP_COVERAGE_RADIUS_MM);
    r * r
}

/// The share of a region's contact cells within reach of a tip (a tip's
/// reach widening with the height above it).
pub fn compute_region_coverage(region: &DetectedIsland, tips: &[Vec3], radius_mm: f64) -> f64 {
    let Some(cells) = region.contact_voxels.as_ref().filter(|v| v.count > 0) else {
        return 0.0;
    };
    if tips.is_empty() {
        return 0.0;
    }
    // Tips by square of the widest reach, so a cell looks at its own square
    // and the eight round it.
    let side = js_max(
        0.5,
        radius_mm
            + js_max(
                0.0,
                influence_radius_mm(f64::INFINITY) - TIP_COVERAGE_RADIUS_MM,
            ),
    );
    let mut squares: HashMap<u64, Vec<&Vec3>> = HashMap::new();
    for tip in tips {
        squares
            .entry(cell_key_bits(
                (tip.x / side).floor(),
                (tip.y / side).floor(),
            ))
            .or_default()
            .push(tip);
    }
    let covered = (0..cells.count)
        .filter(|&i| {
            let (x, y, z) = (cells.x(i), cells.y(i), cells.z(i).unwrap_or(0.0));
            let (sx, sy) = ((x / side).floor(), (y / side).floor());
            (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    squares
                        .get(&cell_key_bits(sx + f64::from(dx), sy + f64::from(dy)))
                        .is_some_and(|near| {
                            near.iter().any(|t| {
                                let (ex, ey) = (x - t.x, y - t.y);
                                ex * ex + ey * ey <= reach2(z, t.z, radius_mm)
                            })
                        })
                })
            })
        })
        .count();
    covered as f64 / cells.count as f64
}

//! DragonFruit `src/supports/autoSupport/coverage.ts`: footprint-aware coverage
//! and the gap-fill candidates the convergence loop adds.

use std::collections::HashMap;

use super::constants::influence_radius_mm;
use crate::supports::types::Vec3;
use crate::geom::math_utils::js_max;
use crate::volume_analysis::islands::types::DetectedIsland;
use crate::volume_analysis::islands::voxel_footprint::cell_key_bits;

/// A tip covers surface within this radius (mm).
pub const TIP_COVERAGE_RADIUS_MM: f64 = 3.0;

/// Effective cover radius for a region of the given footprint area: large flat
/// regions peel harder, so their discs shrink sublinearly, floored at half
/// radius. DragonFruit's default `base_radius_mm` is [`TIP_COVERAGE_RADIUS_MM`].
pub fn coverage_radius_for_area(area_mm2: f64, base_radius_mm: f64) -> f64 {
    if !(area_mm2 > 0.0) {
        return base_radius_mm;
    }
    let scale = (8.0 / js_max(area_mm2, 8.0)).sqrt();
    base_radius_mm * js_max(0.5, scale)
}

/// A tip's coverage disc widens along the influence curve, which caps at 6 mm,
/// so the growth term never adds more than this to a disc's radius.
fn max_influence_growth_mm() -> f64 {
    js_max(
        0.0,
        influence_radius_mm(f64::INFINITY) - TIP_COVERAGE_RADIUS_MM,
    )
}

/// Square of the disc a tip covers at a voxel `vz` above `tip_z`.
fn cover_radius_sq(vz: f64, tip_z: f64, radius_mm: f64) -> f64 {
    let growth = influence_radius_mm(vz - tip_z) - TIP_COVERAGE_RADIUS_MM;
    let r = radius_mm + js_max(0.0, growth);
    r * r
}

/// Tips bucketed on a uniform plan grid; a voxel only tests the tips in its
/// own cell and the eight around it.
struct TipIndex<'a> {
    buckets: HashMap<u64, Vec<&'a Vec3>>,
    cell: f64,
}

impl<'a> TipIndex<'a> {
    fn new(tips: &'a [Vec3], radius_mm: f64) -> Self {
        let cell = js_max(0.5, radius_mm + max_influence_growth_mm());
        let mut buckets: HashMap<u64, Vec<&Vec3>> = HashMap::new();
        for tip in tips {
            let key = cell_key_bits((tip.x / cell).floor(), (tip.y / cell).floor());
            buckets.entry(key).or_default().push(tip);
        }
        TipIndex { buckets, cell }
    }

    /// Whether any tip that could reach (x, y) satisfies `visit`, in bucket
    /// order, stopping at the first.
    fn any_near(&self, x: f64, y: f64, mut visit: impl FnMut(&Vec3) -> bool) -> bool {
        let cx = (x / self.cell).floor();
        let cy = (y / self.cell).floor();
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(bucket) = self
                    .buckets
                    .get(&cell_key_bits(cx + f64::from(dx), cy + f64::from(dy)))
                else {
                    continue;
                };
                if bucket.iter().any(|tip| visit(tip)) {
                    return true;
                }
            }
        }
        false
    }
}

/// Fraction of an overhang region's projected footprint covered by tips. Each
/// tip's disc grows with the voxel height above it (support influence curve).
pub fn compute_region_coverage(region: &DetectedIsland, tips: &[Vec3], radius_mm: f64) -> f64 {
    let Some(voxels) = region.contact_voxels.as_ref().filter(|v| v.count > 0) else {
        return 0.0;
    };
    if tips.is_empty() {
        return 0.0;
    }

    let index = TipIndex::new(tips, radius_mm);
    let mut covered = 0usize;
    for i in 0..voxels.count {
        let vx = voxels.x(i);
        let vy = voxels.y(i);
        let vz = voxels.z(i).unwrap_or(0.0);
        let hit = index.any_near(vx, vy, |tip| {
            let dx = vx - tip.x;
            let dy = vy - tip.y;
            dx * dx + dy * dy <= cover_radius_sq(vz, tip.z, radius_mm)
        });
        if hit {
            covered += 1;
        }
    }
    covered as f64 / voxels.count as f64
}

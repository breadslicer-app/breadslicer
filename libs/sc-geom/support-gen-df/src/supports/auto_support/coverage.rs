//! DragonFruit `src/supports/autoSupport/coverage.ts`: footprint-aware coverage
//! and the gap-fill candidates the convergence loop adds.

use std::collections::{HashMap, HashSet};

use super::constants::influence_radius_mm;
use super::settings::AutoSupportSettings;
use super::types::{CandidatePoint, CandidateSource};
use crate::supports::types::{SupportState, Vec3};
use crate::three::math_utils::{js_max, js_round, js_to_fixed};
use crate::volume_analysis::islands::types::{DetectedIsland, IslandSource};
use crate::volume_analysis::islands::voxel_footprint::cell_key_bits;

/// A tip covers surface within this radius (mm).
pub const TIP_COVERAGE_RADIUS_MM: f64 = 3.0;
/// Region coverage fraction at which no gap-filling is needed.
pub const REGION_COVERAGE_TARGET: f64 = 0.95;
/// Minimum uncovered cluster area (mm²) worth filling.
pub const MIN_GAP_CLUSTER_MM2: f64 = 2.0;
/// Max gap-fill passes per run.
pub const MAX_GAP_FILL_PASSES: usize = 3;

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

/// Collect support-tip world positions from a support snapshot: trunks,
/// branches, leaves and stumps, in that order.
pub fn collect_support_tips(snapshot: &SupportState) -> Vec<Vec3> {
    let c = &snapshot.collections;
    let mut tips = Vec::new();
    // `contactCone?.pos`: an entity without a cone contributes nothing.
    tips.extend(
        c.trunks
            .values()
            .filter_map(|t| t.contact_cone.as_ref().map(|c| c.pos)),
    );
    tips.extend(
        c.branches
            .values()
            .filter_map(|b| b.contact_cone.as_ref().map(|c| c.pos)),
    );
    tips.extend(c.leaves.values().map(|l| l.contact_cone.pos));
    tips.extend(c.stumps.values().map(|s| s.contact_cone.pos));
    tips
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

#[derive(Debug, Clone, Copy)]
struct Cell {
    x: f64,
    y: f64,
    z: Option<f64>,
}

/// Uncovered clusters of an overhang region's footprint (BFS over footprint
/// voxels not covered by any tip): their centroids, with the surface Z of the
/// voxel nearest each centroid. DragonFruit's defaults are
/// [`TIP_COVERAGE_RADIUS_MM`] and [`MIN_GAP_CLUSTER_MM2`].
pub fn find_uncovered_clusters(
    region: &DetectedIsland,
    tips: &[Vec3],
    radius_mm: f64,
    min_cluster_mm2: f64,
) -> Vec<Vec3> {
    let Some(voxels) = region.contact_voxels.as_ref().filter(|v| v.count > 0) else {
        return Vec::new();
    };
    if tips.is_empty() {
        // No tips at all: the caller's main grid handles the region instead.
        return Vec::new();
    }

    let index = TipIndex::new(tips, radius_mm);
    let mut cells: Vec<Cell> = Vec::new();
    for i in 0..voxels.count {
        let x = voxels.x(i);
        let y = voxels.y(i);
        let z = voxels.z(i);
        let covered = index.any_near(x, y, |tip| {
            let dx = x - tip.x;
            let dy = y - tip.y;
            dx * dx + dy * dy <= cover_radius_sq(z.unwrap_or(tip.z), tip.z, radius_mm)
        });
        if !covered {
            cells.push(Cell { x, y, z });
        }
    }

    let mut visited: HashSet<usize> = HashSet::new();
    let mut clusters = Vec::new();
    // Voxels sit on a 0.25 mm grid; exact integer cell key → index (the last
    // voxel on a cell wins, as `Map.set` does).
    let mut index_by_key: HashMap<u64, usize> = HashMap::new();
    for (i, v) in cells.iter().enumerate() {
        index_by_key.insert(cell_key_bits(js_round(v.x * 4.0), js_round(v.y * 4.0)), i);
    }

    for i in 0..cells.len() {
        if visited.contains(&i) {
            continue;
        }
        let mut stack = vec![i];
        visited.insert(i);
        let mut cluster: Vec<Cell> = Vec::new();
        while let Some(idx) = stack.pop() {
            let v = cells[idx];
            cluster.push(v);
            let kx = js_round(v.x * 4.0);
            let ky = js_round(v.y * 4.0);
            for ddx in -1..=1 {
                for ddy in -1..=1 {
                    if ddx == 0 && ddy == 0 {
                        continue;
                    }
                    let Some(&ni) =
                        index_by_key.get(&cell_key_bits(kx + f64::from(ddx), ky + f64::from(ddy)))
                    else {
                        continue;
                    };
                    if !visited.insert(ni) {
                        continue;
                    }
                    stack.push(ni);
                }
            }
        }

        let area_mm2 = cluster.len() as f64 * 0.25 * 0.25;
        if area_mm2 < min_cluster_mm2 {
            continue;
        }

        let (sum_x, sum_y) = cluster
            .iter()
            .fold((0.0, 0.0), |(sx, sy), v| (sx + v.x, sy + v.y));
        let cx = sum_x / cluster.len() as f64;
        let cy = sum_y / cluster.len() as f64;
        let mut best: Option<Cell> = None;
        let mut best_d2 = f64::INFINITY;
        for v in &cluster {
            let d2 = (v.x - cx) * (v.x - cx) + (v.y - cy) * (v.y - cy);
            if d2 < best_d2 {
                best_d2 = d2;
                best = Some(*v);
            }
        }
        clusters.push(Vec3::new(cx, cy, best.and_then(|b| b.z).unwrap_or(0.0)));
    }

    clusters
}

/// Build gap-fill candidates for under-covered overhang regions: standalone
/// trunk candidates (gridPoint) at uncovered footprint cluster centroids, with
/// the region's surface normal.
pub fn build_gap_fill_candidates(
    overhang_islands: &[DetectedIsland],
    settings: &AutoSupportSettings,
    tips: &[Vec3],
) -> Vec<CandidatePoint> {
    let mut out = Vec::new();
    let coverage_target = settings.coverage_target_percent / 100.0;
    for region in overhang_islands {
        if region.source != IslandSource::Overhang {
            continue;
        }
        if region.contact_voxels.as_ref().is_none_or(|v| v.count == 0) {
            continue;
        }
        let radius =
            coverage_radius_for_area(region.area_mm2.unwrap_or(0.0), TIP_COVERAGE_RADIUS_MM);
        if compute_region_coverage(region, tips, radius) >= coverage_target {
            continue;
        }

        for c in find_uncovered_clusters(region, tips, radius, MIN_GAP_CLUSTER_MM2) {
            out.push(CandidatePoint {
                id: format!(
                    "gap-{}-{}-{}",
                    region.id,
                    js_to_fixed(c.x, 2),
                    js_to_fixed(c.y, 2)
                ),
                tip_pos: c,
                tip_normal: region.surface_normal.unwrap_or(Vec3::new(0.0, 0.0, -1.0)),
                model_id: String::new(),
                source: CandidateSource::Overhang,
                island_area_mm2: settings.area_per_support_mm2,
                z_height: c.z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: Some(true),
            });
        }
    }
    out
}

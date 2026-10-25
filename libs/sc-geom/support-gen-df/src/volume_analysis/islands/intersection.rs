//! DragonFruit `src/volumeAnalysis/Islands/intersection.ts`: voxel islands and
//! mesh minima classified against each other (greedy nearest match within an
//! XY radius and a Z band).

use serde::{Deserialize, Serialize};

use super::types::{DetectedIsland, IslandClass};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntersectionOptions {
    pub xy_tolerance_mm: f64,
    pub z_band_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntersectionStats {
    pub voxel_total: usize,
    pub minima_total: usize,
    pub matched: usize,
    pub voxel_only: usize,
    pub minima_only: usize,
    pub minima_superset_of_voxel: bool,
    pub voxel_superset_of_minima: bool,
    pub mean_match_offset_mm: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedIslands {
    /// Copies of the inputs with `class` and `matchedWith` set, voxel then minima.
    pub islands: Vec<DetectedIsland>,
    pub stats: IntersectionStats,
}

/// `classifyIntersection`. Each voxel island claims the closest unused minimum
/// within tolerance and snaps its contact to it. DragonFruit writes
/// `matchedWith: null` on the unmatched; here that is `None`.
pub fn classify_intersection(
    voxel: &[DetectedIsland],
    minima: &[DetectedIsland],
    opts: IntersectionOptions,
) -> ClassifiedIslands {
    let mut v: Vec<DetectedIsland> = voxel.to_vec();
    let mut m: Vec<DetectedIsland> = minima.to_vec();
    let xy_tol2 = opts.xy_tolerance_mm * opts.xy_tolerance_mm;
    let mut minima_used = vec![false; m.len()];
    let mut matched = 0;
    let mut offset_sum = 0.0;

    for vi in &mut v {
        let mut best_idx: Option<usize> = None;
        let mut best_d2 = f64::INFINITY;
        for (j, mj) in m.iter().enumerate() {
            if minima_used[j] {
                continue;
            }
            let dz = vi.base_z - mj.base_z;
            if dz.abs() > opts.z_band_mm {
                continue;
            }
            let dx = vi.contact.x - mj.contact.x;
            let dy = vi.contact.y - mj.contact.y;
            let xy2 = dx * dx + dy * dy;
            if xy2 > xy_tol2 {
                continue;
            }
            let d2 = xy2 + dz * dz;
            if d2 < best_d2 {
                best_d2 = d2;
                best_idx = Some(j);
            }
        }
        match best_idx {
            Some(j) => {
                minima_used[j] = true;
                vi.class = Some(IslandClass::Intersection);
                m[j].class = Some(IslandClass::Intersection);
                vi.matched_with = Some(m[j].id.clone());
                m[j].matched_with = Some(vi.id.clone());
                vi.contact = m[j].contact;
                matched += 1;
                offset_sum += best_d2.sqrt();
            }
            None => {
                vi.class = Some(IslandClass::VoxelOnly);
                vi.matched_with = None;
            }
        }
    }
    for (j, mj) in m.iter_mut().enumerate() {
        if !minima_used[j] {
            mj.class = Some(IslandClass::MinimaOnly);
            mj.matched_with = None;
        }
    }

    let voxel_total = v.len();
    let minima_total = m.len();
    let voxel_only = voxel_total - matched;
    let minima_only = minima_total - matched;
    let stats = IntersectionStats {
        voxel_total,
        minima_total,
        matched,
        voxel_only,
        minima_only,
        minima_superset_of_voxel: voxel_total > 0 && voxel_only == 0,
        voxel_superset_of_minima: minima_total > 0 && minima_only == 0,
        mean_match_offset_mm: if matched > 0 {
            offset_sum / matched as f64
        } else {
            0.0
        },
    };
    v.extend(m);
    ClassifiedIslands { islands: v, stats }
}

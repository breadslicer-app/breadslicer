//! DragonFruit `src/supports/autoSupport/candidateGeneration.ts`: islands to
//! auto-support candidates, scored and deduplicated.

use std::collections::HashMap;

use super::constants::{
    ISLAND_SUB_HEAD_MM, ISLAND_TWO_POINT_MAX_MM, ISLAND_TWO_POINT_MAX_WIDTH_MM,
    ISLAND_TWO_POINT_MIN_MM, SMALL_ISLAND_TIP_AREA_MM2, SUPPORT_RESTSTACK_DELTA_MM,
    influence_radius_mm,
};
use super::contact_tip_cap::apply_contact_tip_caps;
use super::coverage::TIP_COVERAGE_RADIUS_MM;
use super::parameter_sizing::SizingBands;
use super::settings::AutoSupportSettings;
use super::support_blockers::is_support_blocked_contact;
use super::types::{AutoSupportMesh, CandidatePoint, CandidateSource, js_compare};
use crate::supports::settings::types::SupportSettings;
use crate::supports::types::Vec3;
use crate::three::math_utils::{js_max, js_min, js_round};
use crate::volume_analysis::islands::types::{DetectedIsland, IslandClass, IslandSource};

/// Convert detected islands into auto-support candidate points: filtered,
/// pruned against the support-blocker mask, tip-capped, scored and sorted by
/// priority, highest first. `mesh` is DragonFruit's prune context; the mask is
/// consulted only when the model has one.
pub fn generate_candidates(
    islands: &[DetectedIsland],
    auto: &AutoSupportSettings,
    mesh: Option<AutoSupportMesh<'_>>,
    settings: &SupportSettings,
    bands: &SizingBands,
) -> Vec<CandidatePoint> {
    if islands.is_empty() {
        return Vec::new();
    }

    // A minima is a sharp geometric feature that needs support regardless of
    // size, and it carries no area of its own: the exemption reads the source.
    let eligible = islands.iter().filter(|island| {
        island.source == IslandSource::Minima
            || island.area_mm2.unwrap_or(0.0) >= auto.min_island_area_mm2
    });

    let mut candidates: Vec<CandidatePoint> = eligible
        .flat_map(|island| candidates_from_island(island, bands))
        .collect();
    // Support blockers: refuse contacts painted as nogo.
    if let Some(m) = mesh {
        candidates.retain(|c| {
            !is_support_blocked_contact(m.blocked, m.mesh.rays, m.side, c.tip_pos, None)
        });
    }
    // Cap the tip contact by the local free width at each contact.
    apply_contact_tip_caps(&mut candidates, mesh.map(|m| m.mesh.field), settings, bands);
    if candidates.is_empty() {
        return Vec::new();
    }
    let max_z = candidates.iter().fold(1.0, |m, c| js_max(m, c.z_height));
    let max_area = candidates
        .iter()
        .fold(0.01, |m, c| js_max(m, c.island_area_mm2));
    for c in &mut candidates {
        c.priority = compute_priority(c, max_z, max_area, auto);
    }

    candidates.sort_by(|a, b| js_compare(b.priority - a.priority));
    candidates
}

/// A candidate from a single island. The normal and model id are
/// placeholders the caller fills in.
pub fn candidate_from_island(island: &DetectedIsland, bands: &SizingBands) -> CandidatePoint {
    // Minima islands don't have an area: a default scores them alongside voxels.
    let area = island
        .area_mm2
        .unwrap_or(if island.source == IslandSource::Minima {
            0.05
        } else {
            0.0
        });
    let source = if island.class == Some(IslandClass::Intersection) {
        CandidateSource::Intersection
    } else {
        match island.source {
            IslandSource::Voxel => CandidateSource::Voxel,
            IslandSource::Minima => CandidateSource::Minima,
            IslandSource::Overhang => CandidateSource::Overhang,
        }
    };

    CandidatePoint {
        id: island.id.clone(),
        tip_pos: island.contact,
        tip_normal: Vec3::new(0.0, 0.0, -1.0),
        model_id: String::new(),
        source,
        island_area_mm2: area,
        z_height: island.base_z,
        priority: 0.0,
        // Fine detail keeps a shrunk (detail-band) tip.
        tip_diameter_mm: (area < SMALL_ISLAND_TIP_AREA_MM2)
            .then(|| bands.small_island_tip_diameter_mm()),
        grid_point: None,
        zone_size: None,
    }
}

/// Island-typed emission: one island yields one or two candidates by span.
/// Sub-head specks get a single tip at the bbox center; narrow islands in the
/// two-point band split into a symmetric pair, each carrying half the area.
pub fn candidates_from_island(island: &DetectedIsland, bands: &SizingBands) -> Vec<CandidatePoint> {
    let base = candidate_from_island(island, bands);
    let Some(voxels) = island.contact_voxels.as_ref().filter(|v| v.count > 0) else {
        return vec![base];
    };
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for i in 0..voxels.count {
        let x = voxels.x(i);
        let y = voxels.y(i);
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }
    let ext_x = max_x - min_x;
    let ext_y = max_y - min_y;
    let major = js_max(ext_x, ext_y);
    if major <= ISLAND_SUB_HEAD_MM {
        return vec![CandidatePoint {
            tip_pos: Vec3::new(
                (min_x + max_x) / 2.0,
                (min_y + max_y) / 2.0,
                island.contact.z,
            ),
            ..base
        }];
    }
    let minor = js_min(ext_x, ext_y);
    if (ISLAND_TWO_POINT_MIN_MM..ISLAND_TWO_POINT_MAX_MM).contains(&major)
        && minor < ISLAND_TWO_POINT_MAX_WIDTH_MM
    {
        let along_x = ext_x >= ext_y;
        let lo = if along_x { min_x } else { min_y };
        let span = if along_x { ext_x } else { ext_y };
        let fixed = if along_x {
            (min_y + max_y) / 2.0
        } else {
            (min_x + max_x) / 2.0
        };
        let half_area = base.island_area_mm2 / 2.0;
        let pair_tip =
            (half_area < SMALL_ISLAND_TIP_AREA_MM2).then(|| bands.small_island_tip_diameter_mm());
        return [0.25, 0.75]
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let along = lo + span * t;
                CandidatePoint {
                    id: format!("{}-{}", island.id, if i == 0 { 'a' } else { 'b' }),
                    tip_pos: if along_x {
                        Vec3::new(along, fixed, island.contact.z)
                    } else {
                        Vec3::new(fixed, along, island.contact.z)
                    },
                    island_area_mm2: half_area,
                    tip_diameter_mm: pair_tip,
                    ..base.clone()
                }
            })
            .collect();
    }
    vec![base]
}

/// Placement priority, higher = more urgent: 60% area, 30% Z-height (lower =
/// more urgent), 10% source bonus.
fn compute_priority(
    c: &CandidatePoint,
    max_z: f64,
    max_area: f64,
    auto: &AutoSupportSettings,
) -> f64 {
    let area_score = (c.island_area_mm2 / js_max(max_area, 0.01)) * 0.6;
    let z_score = (1.0 - c.z_height / js_max(max_z, 1.0)) * 0.3;
    let source_score = if c.source == CandidateSource::Intersection {
        0.1
    } else {
        0.0
    };
    let mut priority = area_score + z_score + source_score;
    if auto.prioritize_intersection && c.source == CandidateSource::Intersection {
        priority *= 1.5;
    }
    priority
}

/// A `Math.round` cell coordinate as the `${cx},${cy}` key reads it back
/// (`String(-0)` is "0").
fn cell_coord(v: f64) -> i64 {
    js_round(v) as i64
}

/// Deduplicate candidates using a spatial hash grid. Overhang-lattice pairs
/// suppress on an XY disc that widens with vertical separation (support
/// influence curve), except across the restack allowance, so staircase
/// shelves keep their supports. Discrete islands (voxel/minima/intersection)
/// always use the flat 3D ball: neighboring islands must never eat each other.
pub fn deduplicate_candidates(
    candidates: &[CandidatePoint],
    auto: &AutoSupportSettings,
) -> Vec<CandidatePoint> {
    if candidates.len() <= 1 {
        return candidates.to_vec();
    }

    let base_radius = auto.tip_influence_radius_mm;
    if base_radius <= 0.0 {
        let mut sorted = candidates.to_vec();
        sorted.sort_by(|a, b| js_compare(b.priority - a.priority));
        return sorted;
    }
    let max_gate = base_radius + (6.0 - TIP_COVERAGE_RADIUS_MM);
    let cell_size = base_radius;
    let cell_range = js_max(1.0, (max_gate / cell_size).ceil()) as i64;

    let cell_of = |c: &CandidatePoint| {
        (
            cell_coord(c.tip_pos.x / cell_size),
            cell_coord(c.tip_pos.y / cell_size),
        )
    };
    let mut grid: HashMap<(i64, i64), Vec<&CandidatePoint>> = HashMap::new();
    for c in candidates {
        grid.entry(cell_of(c)).or_default().push(c);
    }

    let mut sorted: Vec<&CandidatePoint> = candidates.iter().collect();
    sorted.sort_by(|a, b| js_compare(b.priority - a.priority));
    let mut retained: Vec<&CandidatePoint> = Vec::new();

    for c in sorted {
        let (cx, cy) = cell_of(c);
        let mut duplicate = false;
        'cells: for dx in -cell_range..=cell_range {
            for dy in -cell_range..=cell_range {
                let Some(bucket) = grid.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for r in &retained {
                    // Only candidates bucketed here can be this close.
                    if !bucket.iter().any(|rr| rr.id == r.id) {
                        continue;
                    }
                    let ddx = c.tip_pos.x - r.tip_pos.x;
                    let ddy = c.tip_pos.y - r.tip_pos.y;
                    let dz = (c.tip_pos.z - r.tip_pos.z).abs();
                    if c.source != CandidateSource::Overhang
                        || r.source != CandidateSource::Overhang
                    {
                        let ddz = c.tip_pos.z - r.tip_pos.z;
                        if ddx * ddx + ddy * ddy + ddz * ddz <= base_radius * base_radius {
                            duplicate = true;
                            break 'cells;
                        }
                        continue;
                    }
                    // Restack: vertically separated shelves never suppress each other.
                    if dz > SUPPORT_RESTSTACK_DELTA_MM {
                        continue;
                    }
                    let gate =
                        base_radius + js_max(0.0, influence_radius_mm(dz) - TIP_COVERAGE_RADIUS_MM);
                    if ddx * ddx + ddy * ddy <= gate * gate {
                        duplicate = true;
                        break 'cells;
                    }
                }
            }
        }

        if !duplicate {
            retained.push(c);
        }
    }

    let mut out: Vec<CandidatePoint> = retained.into_iter().cloned().collect();
    out.sort_by(|a, b| js_compare(b.priority - a.priority));
    out
}

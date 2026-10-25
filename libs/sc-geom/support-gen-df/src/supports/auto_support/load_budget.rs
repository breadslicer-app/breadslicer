//! DragonFruit `src/supports/autoSupport/loadBudget.ts`.
//!
//! The load budget: how much surface each island's contacts are responsible
//! for, against what they can carry, as a report only. Units are mm² of
//! unsupported surface on both sides of the subtraction:
//!
//! - **Demand** is the island's own footprint area, plus a share of the demand
//!   of the islands it sits under, plus, when the pose needs topple coverage, a
//!   share of the forest's demand proportional to the drag moment the patch
//!   carries.
//! - **Capacity** credits each placed support with the area the run's density
//!   knob already assigns one support, scaled by its band's cross-section
//!   relative to the structure band.
//!
//! What it is not: a simulation. It reports instead of placing.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::parameter_sizing::SizingBands;
use super::settings::SizingPreset;
use crate::supports::types::Vec3;
use crate::three::math_utils::{js_max, js_min, js_round};
use crate::volume_analysis::islands::types::DetectedIsland;
use crate::volume_analysis::islands::voxel_footprint::number_key;

/// Footprint cells are the detector's own 0.25 mm grid.
const FOOTPRINT_CELL_MM: f64 = 0.25;
/// Rise above another island below which two islands are treated as one level.
const LEVEL_EPSILON_MM: f64 = 0.05;
/// Within this plan distance of a footprint, a contact counts as serving it.
const CONTACT_ASSIGNMENT_MM: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlacedContact {
    pub tip: Vec3,
    /// The tier the support's island falls in, for capacity.
    pub preset: SizingPreset,
}

pub struct LoadBudgetInput<'a> {
    pub islands: &'a [DetectedIsland],
    pub contacts: &'a [PlacedContact],
    /// From the settings the run used: the area one support is expected to cover.
    pub area_per_support_mm2: f64,
    /// The pose's total drag moment (mm³), when the scan reported one.
    pub pose_drag_moment_mm3: Option<f64>,
    /// Whether the pose's verdict asked for anti-topple coverage at all.
    pub topple_coverage_needed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadBudgetIsland {
    pub island_id: String,
    pub demand_mm2: f64,
    pub capacity_mm2: f64,
    pub deficit_mm2: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadBudgetReport {
    /// One row per island with an area of its own, in island order.
    pub rows: Vec<LoadBudgetIsland>,
    pub total_demand_mm2: f64,
    pub total_capacity_mm2: f64,
    pub would_add: f64,
    pub would_cull: f64,
    pub islands_in_deficit: usize,
    pub islands_in_surplus: usize,
    pub islands_without_area: usize,
    pub worst: Option<LoadBudgetIsland>,
}

/// A footprint cell as JavaScript's numeric key.
fn cell(x: f64, y: f64) -> f64 {
    (js_round(x / FOOTPRINT_CELL_MM) + 32768.0) * 65536.0
        + (js_round(y / FOOTPRINT_CELL_MM) + 32768.0)
}

/// Index of the footprint cells one island occupies, or None when it carries none.
fn footprint_cells(island: &DetectedIsland) -> Option<HashSet<u64>> {
    let footprint = island.contact_voxels.as_ref()?;
    if footprint.count == 0 {
        return None;
    }
    Some(
        (0..footprint.count)
            .map(|i| number_key(cell(footprint.x(i), footprint.y(i))))
            .collect(),
    )
}

/// How many of `cells` fall inside `other`, as a fraction of `cells`.
fn overlap_fraction(cells: &HashSet<u64>, other: &HashSet<u64>) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let shared = cells.iter().filter(|c| other.contains(c)).count();
    shared as f64 / cells.len() as f64
}

/// A support's capacity: the area one support is expected to cover, credited
/// by the cross-section of the tier its island falls in (a relative weight).
fn capacity_for(tier: SizingPreset, area_per_support_mm2: f64, bands: &SizingBands) -> f64 {
    let baseline = bands.resolve(Some("structure")).shaft_diameter_mm;
    let band = bands.resolve(Some(tier.as_str())).shaft_diameter_mm;
    let ratio = band / baseline;
    area_per_support_mm2 * (ratio * ratio)
}

/// Chebyshev distance in mm from a cell to the nearest cell of a set.
fn nearest_cell_distance_mm(cells: &HashSet<u64>, cell: f64) -> f64 {
    let x = (cell / 65536.0).floor() - 32768.0;
    let y = (cell % 65536.0) - 32768.0;
    let mut best = f64::INFINITY;
    for &bits in cells {
        let other = f64::from_bits(bits);
        let ox = (other / 65536.0).floor() - 32768.0;
        let oy = (other % 65536.0) - 32768.0;
        let distance = js_max((ox - x).abs(), (oy - y).abs()) * FOOTPRINT_CELL_MM;
        if distance < best {
            best = distance;
        }
        if best == 0.0 {
            break;
        }
    }
    best
}

/// Areas and the load path are stated per island, and every island keeps its
/// own row even when it ends up balanced.
pub fn compute_load_budget(input: &LoadBudgetInput<'_>, bands: &SizingBands) -> LoadBudgetReport {
    let area = |island: &DetectedIsland| island.area_mm2.unwrap_or(0.0);
    let with_area: Vec<&DetectedIsland> = input.islands.iter().filter(|i| area(i) > 0.0).collect();
    let islands_without_area = input.islands.len() - with_area.len();
    let forest_demand_mm2 = with_area.iter().fold(0.0, |sum, i| sum + area(i));

    // Demand, top down: an island's own area plus the share of what sits above
    // it that lands on its footprint.
    let mut demand_by_id: IndexMap<&str, f64> = IndexMap::new();
    for island in &with_area {
        demand_by_id.insert(&island.id, area(island));
    }

    let cells_by_id: IndexMap<&str, Option<HashSet<u64>>> = with_area
        .iter()
        .map(|i| (i.id.as_str(), footprint_cells(i)))
        .collect();
    let with_cells: Vec<&DetectedIsland> = with_area
        .iter()
        .copied()
        .filter(|i| cells_by_id[i.id.as_str()].is_some())
        .collect();
    let mut descending = with_cells.clone();
    descending.sort_by(|a, b| {
        (b.base_z - a.base_z)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for upper in &descending {
        if upper.base_z <= LEVEL_EPSILON_MM {
            continue;
        }
        let upper_cells = cells_by_id[upper.id.as_str()].as_ref().unwrap();
        let carried = demand_by_id.get(upper.id.as_str()).copied().unwrap_or(0.0);
        for lower in &with_cells {
            if lower.id == upper.id {
                continue;
            }
            if lower.base_z >= upper.base_z - LEVEL_EPSILON_MM {
                continue;
            }
            let fraction = overlap_fraction(
                upper_cells,
                cells_by_id[lower.id.as_str()].as_ref().unwrap(),
            );
            if fraction <= 0.0 {
                continue;
            }
            let entry = demand_by_id.entry(&lower.id).or_insert(0.0);
            *entry += carried * fraction;
        }
    }

    // Topple channel: a patch that carries drag is charged a share of the
    // forest's demand, and only when the pose's verdict asked for coverage.
    if let Some(pose_drag) = input.pose_drag_moment_mm3
        && input.topple_coverage_needed
        && pose_drag > 0.0
    {
        for island in &with_area {
            let drag = island.drag_moment_mm3.unwrap_or(0.0);
            if drag <= 0.0 {
                continue;
            }
            let share = js_min(1.0, drag / pose_drag);
            let entry = demand_by_id.entry(&island.id).or_insert(0.0);
            *entry += forest_demand_mm2 * share;
        }
    }

    // Capacity: each placed contact is credited to the island whose footprint
    // it landed on (rarely, to the nearest one within a contact radius).
    let mut capacity_by_id: IndexMap<&str, f64> =
        with_area.iter().map(|i| (i.id.as_str(), 0.0)).collect();
    for contact in input.contacts {
        let contact_cell = cell(contact.tip.x, contact.tip.y);
        let mut owner: Option<&DetectedIsland> = None;
        let mut best_score_mm = f64::INFINITY;
        for island in &with_area {
            let Some(cells) = cells_by_id[island.id.as_str()].as_ref() else {
                continue;
            };
            let distance_mm = if cells.contains(&number_key(contact_cell)) {
                0.0
            } else {
                nearest_cell_distance_mm(cells, contact_cell)
            };
            if distance_mm > CONTACT_ASSIGNMENT_MM {
                continue;
            }
            // A contact belongs to the surface it landed on, not to the
            // platform under it: plan distance decides, height breaks the tie.
            let height_mm = (island.base_z - contact.tip.z).abs();
            let score = distance_mm + height_mm;
            if score < best_score_mm {
                best_score_mm = score;
                owner = Some(island);
            }
        }
        let Some(owner) = owner else { continue };
        let entry = capacity_by_id.entry(&owner.id).or_insert(0.0);
        *entry += capacity_for(contact.preset, input.area_per_support_mm2, bands);
    }

    let structure_capacity =
        capacity_for(SizingPreset::Structure, input.area_per_support_mm2, bands);
    let rows: Vec<LoadBudgetIsland> = with_area
        .iter()
        .map(|island| {
            let demand_mm2 = demand_by_id.get(island.id.as_str()).copied().unwrap_or(0.0);
            let capacity_mm2 = capacity_by_id
                .get(island.id.as_str())
                .copied()
                .unwrap_or(0.0);
            LoadBudgetIsland {
                island_id: island.id.clone(),
                demand_mm2,
                capacity_mm2,
                deficit_mm2: demand_mm2 - capacity_mm2,
            }
        })
        .collect();

    let in_deficit: Vec<&LoadBudgetIsland> = rows
        .iter()
        .filter(|r| r.deficit_mm2 > structure_capacity * 0.5)
        .collect();
    let in_surplus: Vec<&LoadBudgetIsland> = rows
        .iter()
        .filter(|r| r.deficit_mm2 < -structure_capacity * 0.5)
        .collect();
    let worst = in_deficit
        .iter()
        .fold(None::<&LoadBudgetIsland>, |current, row| match current {
            Some(c) if !(row.deficit_mm2 > c.deficit_mm2) => Some(c),
            _ => Some(row),
        })
        .cloned();

    LoadBudgetReport {
        total_demand_mm2: rows.iter().fold(0.0, |s, r| s + r.demand_mm2),
        total_capacity_mm2: rows.iter().fold(0.0, |s, r| s + r.capacity_mm2),
        would_add: in_deficit
            .iter()
            .fold(0.0, |s, r| s + (r.deficit_mm2 / structure_capacity).ceil()),
        would_cull: in_surplus.iter().fold(0.0, |s, r| {
            s + (-r.deficit_mm2 / structure_capacity).floor()
        }),
        islands_in_deficit: in_deficit.len(),
        islands_in_surplus: in_surplus.len(),
        islands_without_area,
        worst,
        rows,
    }
}

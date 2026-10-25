//! DragonFruit `src/volumeAnalysis/Islands/useIslands.ts`, the derivation the
//! hook's memo chain runs between the native scans and auto-support: scan
//! results mapped to islands, optional voxel consolidation, voxel × minima
//! classification with the contour threshold, the overhang merge, the
//! existing-support annotation and the default filter. The React state, the
//! overlay markers and the scan cache are the GUI's.
//!
//! The native results reach DragonFruit's frontend over IPC as JSON, so every
//! `f32` arrives as its shortest decimal read back as a JS number; [`ipc32`]
//! does that conversion, and the islands carry exactly the values the hook sees.

use std::collections::HashSet;

use indexmap::IndexMap;

use super::filtering::{
    IslandFilterInput, IslandFilterToggles, TipInfo, annotate_filter_flags, apply_filter,
};
use super::intersection::{IntersectionOptions, IntersectionStats, classify_intersection};
use super::spatial_hash_grid_2d::{SpatialHashGrid2D, cell_key};
use super::types::{DetectedIsland, IslandClass, IslandSource};
use super::voxel_footprint::{
    VoxelFootprint, VoxelFootprintBuilder, concat_footprints, is_empty_footprint, number_key,
};
use crate::native::mesh_minima::CombinedIslandScanResult;
use crate::native::overhang::{OverhangRegion, OverhangScan};
use crate::supports::auto_support::pose_stability::CONSERVATIVE_P_SIGMA;
use crate::supports::auto_support::types::js_compare;
use crate::supports::types::Vec3;
use crate::three::math_utils::{js_max, js_min, js_round};

/// An `f32` as the frontend receives it: serde_json writes it (ryu's shortest
/// decimal, whose tie choice can differ from `Display`'s) and JavaScript parses
/// that as a double. A non-finite value is written as `null`, read as NaN here.
pub fn ipc32(v: f32) -> f64 {
    if v.is_finite() {
        serde_json::to_string(&v)
            .expect("a finite f32 serialises")
            .parse()
            .expect("serde_json's own text")
    } else {
        f64::NAN
    }
}

fn ipc_vec3(v: crate::native::mesh_core::mesh::Vec3) -> Vec3 {
    Vec3::new(ipc32(v.x), ipc32(v.y), ipc32(v.z))
}

/// A non-finite `f64` crosses IPC as `null`, which a JavaScript comparison or
/// `?? 0` reads as 0.
fn ipc_null_as_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// `scanNeedsCoverage`: no report reads as "needs coverage", and neither does
/// an undefined adhesion ratio (`null < p/σ` holds in JavaScript).
pub fn scan_needs_coverage(scan: &OverhangScan) -> bool {
    scan.stability
        .as_ref()
        .is_none_or(|s| ipc_null_as_zero(s.adhesion_ratio) < CONSERVATIVE_P_SIGMA)
}

/// `overhangRegionToIsland`: contact on the footprint pixel with the lowest
/// sampled surface Z (the mask centre at `minZ` when none is usable).
pub fn overhang_region_to_island(region: &OverhangRegion, i: usize) -> DetectedIsland {
    let f = &region.footprint;
    let (width, height) = (f.width as usize, f.height as usize);
    let (origin_x, origin_y, px_mm) = (ipc32(f.origin_x), ipc32(f.origin_y), ipc32(f.px_mm));
    let mut contact_voxels = VoxelFootprintBuilder::new(true);
    let mut best_idx: Option<usize> = None;
    let mut best_z = f64::INFINITY;
    for row in 0..height {
        for col in 0..width {
            let idx = row * width + col;
            if f.data[idx] != 0 {
                let x = origin_x + (col as f64 + 0.5) * px_mm;
                let y = origin_y + (row as f64 + 0.5) * px_mm;
                // A non-finite surface Z arrives as null: stored as 0, never chosen.
                let z = f.surface_z[idx];
                let z = z.is_finite().then(|| ipc32(z));
                contact_voxels.push(x, y, z);
                if let Some(z) = z
                    && z < best_z
                {
                    best_z = z;
                    best_idx = Some(idx);
                }
            }
        }
    }
    let (contact_x, contact_y, contact_z) = match best_idx {
        Some(idx) => (
            origin_x + ((idx % width) as f64 + 0.5) * px_mm,
            origin_y + ((idx / width) as f64 + 0.5) * px_mm,
            best_z,
        ),
        None => (
            origin_x + (width as f64 * px_mm) / 2.0,
            origin_y + (height as f64 * px_mm) / 2.0,
            ipc32(region.min_z),
        ),
    };
    DetectedIsland {
        id: format!("o{i}"),
        source: IslandSource::Overhang,
        contact: Vec3::new(contact_x, contact_y, contact_z),
        base_z: contact_z,
        area_mm2: Some(ipc32(region.projected_area_mm2)),
        overhang_angle_deg: Some(ipc32(region.angle_deg)),
        surface_area_mm2: Some(ipc32(region.area_mm2)),
        max_z: Some(ipc32(region.max_z)),
        drag_moment_mm3: Some(ipc32(region.drag_moment_mm3)),
        drag_dir_deg: Some(ipc32(region.drag_dir_deg)),
        steep_flat: Some(region.steep_flat),
        triangle_ids: Some(region.triangle_ids.iter().map(|&t| t as usize).collect()),
        surface_normal: Some(Vec3::new(
            ipc32(region.normal[0]),
            ipc32(region.normal[1]),
            ipc32(region.normal[2]),
        )),
        contact_voxels: Some(contact_voxels.build()),
        perimeter_loops: Some(
            region
                .perimeter_loops
                .iter()
                .map(|l| {
                    l.iter()
                        .map(|p| [ipc32(p[0]), ipc32(p[1]), ipc32(p[2])])
                        .collect()
                })
                .collect(),
        ),
        ..DetectedIsland::default()
    }
}

/// `overhangCoversVoxel`: the voxel contact inside the region's footprint
/// bounds, with comparable areas.
fn overhang_covers_voxel(region: &DetectedIsland, voxel: &DetectedIsland) -> bool {
    let Some(vox) = region.contact_voxels.as_ref().filter(|v| v.count > 0) else {
        return false;
    };
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for i in 0..vox.count {
        let (px, py) = (vox.x(i), vox.y(i));
        if px < min_x {
            min_x = px;
        }
        if py < min_y {
            min_y = py;
        }
        if px > max_x {
            max_x = px;
        }
        if py > max_y {
            max_y = py;
        }
    }
    let (cx, cy) = (voxel.contact.x, voxel.contact.y);
    if cx < min_x || cx > max_x || cy < min_y || cy > max_y {
        return false;
    }
    let region_area = region.area_mm2.unwrap_or(0.0);
    let voxel_area = voxel.area_mm2.unwrap_or(0.0);
    if region_area <= 0.0 {
        return false;
    }
    let ratio = voxel_area / region_area;
    (0.5..=2.0).contains(&ratio)
}

/// `mergeOverhangRegions`: voxel islands an overhang region covers give way to
/// the region; the regions follow the rest.
pub fn merge_overhang_regions(
    classified: Vec<DetectedIsland>,
    overhang: &[DetectedIsland],
) -> Vec<DetectedIsland> {
    if overhang.is_empty() {
        return classified;
    }
    let covered: HashSet<String> = classified
        .iter()
        .filter(|v| {
            v.source == IslandSource::Voxel && overhang.iter().any(|o| overhang_covers_voxel(o, v))
        })
        .map(|v| v.id.clone())
        .collect();
    let mut out: Vec<DetectedIsland> = classified
        .into_iter()
        .filter(|v| !covered.contains(&v.id))
        .collect();
    out.extend(overhang.iter().cloned());
    out
}

/// `dilateVoxelGrid`: the footprint snapped to its pixel grid and grown by a
/// disc of half the consolidation distance (at least one pixel).
fn dilate_voxel_grid(
    voxels: &VoxelFootprint,
    px_mm: f64,
    consolidation_distance: f64,
) -> VoxelFootprint {
    if voxels.count == 0 {
        return voxels.clone();
    }
    let mut grid_set: HashSet<u64> = HashSet::new();
    let mut original: Vec<(f64, f64)> = Vec::new();
    for i in 0..voxels.count {
        let ix = js_round(voxels.x(i) / px_mm);
        let iy = js_round(voxels.y(i) / px_mm);
        if grid_set.insert(number_key(cell_key(ix, iy))) {
            original.push((ix, iy));
        }
    }
    let r_pix = js_round(consolidation_distance / (2.0 * px_mm)).max(1.0);
    let r = r_pix as i64;
    let mut offsets: Vec<(f64, f64)> = Vec::new();
    for dx in -r..=r {
        for dy in -r..=r {
            if (dx * dx + dy * dy) as f64 <= r_pix * r_pix {
                offsets.push((dx as f64, dy as f64));
            }
        }
    }
    let mut dilated_set: HashSet<u64> = HashSet::new();
    let mut dilated = VoxelFootprintBuilder::new(false);
    for &(ix, iy) in &original {
        for &(dx, dy) in &offsets {
            let (nix, niy) = (ix + dx, iy + dy);
            if dilated_set.insert(number_key(cell_key(nix, niy))) {
                dilated.push(nix * px_mm, niy * px_mm, None);
            }
        }
    }
    dilated.build()
}

const MIN_AREA_FOR_CONTOUR: f64 = 0.06;

/// `consolidateVoxelIslands`: voxel islands within `epsilon_mm` of each other
/// join one cluster, sited on its lowest member; clusters without a member of
/// contour size stay separate. Every island keeps its members.
pub fn consolidate_voxel_islands(
    islands: &[DetectedIsland],
    epsilon_mm: f64,
    px_mm: f64,
) -> Vec<DetectedIsland> {
    let n = islands.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        let mut single = islands[0].clone();
        if single.area_mm2.unwrap_or(0.0) >= MIN_AREA_FOR_CONTOUR
            && !is_empty_footprint(single.contact_voxels.as_ref())
        {
            let dilated = dilate_voxel_grid(
                single.contact_voxels.as_ref().expect("checked"),
                px_mm,
                epsilon_mm,
            );
            single.area_mm2 = Some(dilated.count as f64 * px_mm * px_mm);
            single.contact_voxels = Some(dilated);
        }
        single.members = Some(vec![islands[0].clone()]);
        return vec![single];
    }

    let eps2 = epsilon_mm * epsilon_mm;
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        while parent[x] != root {
            let next = parent[x];
            parent[x] = root;
            x = next;
        }
        root
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (islands[i].contact, islands[j].contact);
            let d2 =
                (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) + (a.z - b.z) * (a.z - b.z);
            if d2 <= eps2 {
                let ra = find(&mut parent, i);
                let rb = find(&mut parent, j);
                parent[ra] = rb;
            }
        }
    }
    let mut by_root: IndexMap<usize, Vec<DetectedIsland>> = IndexMap::new();
    for (i, island) in islands.iter().enumerate() {
        let root = find(&mut parent, i);
        by_root.entry(root).or_default().push(island.clone());
    }

    let mut consolidated = Vec::new();
    for (_, mut members) in by_root {
        if members
            .iter()
            .any(|m| m.area_mm2.unwrap_or(0.0) >= MIN_AREA_FOR_CONTOUR)
        {
            members.sort_by(|a, b| js_compare(a.base_z - b.base_z));
            let lowest = members[0].clone();
            let mut total_area = 0.0;
            let (mut min_first, mut max_last) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut footprints: Vec<&VoxelFootprint> = Vec::new();
            for m in &members {
                total_area += m.area_mm2.unwrap_or(0.0);
                if let Some([a, b]) = m.layer_span {
                    min_first = js_min(min_first, a);
                    max_last = js_max(max_last, b);
                }
                if let Some(v) = &m.contact_voxels {
                    footprints.push(v);
                }
            }
            let merged = concat_footprints(&footprints);
            let dilated = (merged.count > 0).then(|| dilate_voxel_grid(&merged, px_mm, epsilon_mm));
            let final_area = match &dilated {
                Some(d) if d.count > 0 => d.count as f64 * px_mm * px_mm,
                _ => total_area,
            };
            consolidated.push(DetectedIsland {
                area_mm2: Some(final_area),
                layer_span: (min_first != f64::INFINITY).then_some([min_first, max_last]),
                contact_voxels: dilated,
                members: Some(members.clone()),
                ..lowest
            });
        } else {
            for m in members {
                let mut one = m.clone();
                one.members = Some(vec![m]);
                consolidated.push(one);
            }
        }
    }
    consolidated
}

/// `determineContourThreshold`: which consolidated islands keep their merged
/// contour — all qualified ones up to the cap, else the top K at the sharpest
/// area drop-off.
pub fn determine_contour_threshold(
    islands: &[DetectedIsland],
    max_contour_regions: usize,
) -> HashSet<String> {
    let mut contoured = HashSet::new();
    let candidates: Vec<&DetectedIsland> = islands
        .iter()
        .filter(|i| {
            matches!(
                i.class,
                Some(IslandClass::VoxelOnly | IslandClass::Intersection)
            ) && !is_empty_footprint(i.contact_voxels.as_ref())
        })
        .collect();
    if candidates.is_empty() {
        return contoured;
    }
    let mut sorted: Vec<&DetectedIsland> = candidates
        .into_iter()
        .filter(|i| i.area_mm2.unwrap_or(0.0) >= MIN_AREA_FOR_CONTOUR)
        .collect();
    sorted.sort_by(|a, b| js_compare(b.area_mm2.unwrap_or(0.0) - a.area_mm2.unwrap_or(0.0)));
    if sorted.is_empty() {
        return contoured;
    }
    if sorted.len() <= max_contour_regions {
        contoured.extend(sorted.iter().map(|i| i.id.clone()));
        return contoured;
    }
    let areas: Vec<f64> = sorted.iter().map(|i| i.area_mm2.unwrap_or(0.0)).collect();
    let total_area = areas.iter().fold(0.0, |s, a| s + a);
    let mut cumulative = 0.0;
    let mut cum_index = 0;
    for (i, a) in areas.iter().enumerate() {
        cumulative += a;
        if cumulative >= total_area * 0.90 {
            cum_index = i;
            break;
        }
    }
    let min_k = 5.min(sorted.len());
    let max_k = max_contour_regions.min(sorted.len());
    let mut best_k = max_k.min(min_k.max(cum_index + 1));
    let mut max_drop = -1.0;
    for k in min_k..=max_k {
        if k < areas.len() {
            let (prev, curr) = (areas[k - 1], areas[k]);
            if prev > 0.0 {
                let drop = (prev - curr) / prev;
                if drop > max_drop {
                    max_drop = drop;
                    best_k = k;
                }
            }
        }
    }
    contoured.extend(sorted[..best_k].iter().map(|i| i.id.clone()));
    contoured
}

#[derive(Debug, Clone, Copy)]
struct IslandGridEntry {
    island_index: usize,
    x: f64,
    y: f64,
    z: f64,
}

/// `buildIslandContactGrid`: every contact voxel (or the contact) of every
/// island, at the island's contact height.
fn build_island_contact_grid(islands: &[DetectedIsland]) -> SpatialHashGrid2D<IslandGridEntry> {
    let mut grid = SpatialHashGrid2D::new(1.0);
    for (island_index, island) in islands.iter().enumerate() {
        let z = island.contact.z;
        match island.contact_voxels.as_ref().filter(|f| f.count > 0) {
            Some(f) => {
                for i in 0..f.count {
                    let (x, y) = (f.x(i), f.y(i));
                    grid.insert(
                        x,
                        y,
                        IslandGridEntry {
                            island_index,
                            x,
                            y,
                            z,
                        },
                    );
                }
            }
            None => grid.insert(
                island.contact.x,
                island.contact.y,
                IslandGridEntry {
                    island_index,
                    x: island.contact.x,
                    y: island.contact.y,
                    z,
                },
            ),
        }
    }
    grid
}

/// `annotateAndCountSupports`: the filter flags, then per-island counts of the
/// tips touching any of its voxels, `supported` and `fullySupported`.
fn annotate_and_count_supports(
    islands: &[DetectedIsland],
    grid: &SpatialHashGrid2D<IslandGridEntry>,
    support_tips: &[TipInfo],
    plate_z: f64,
    area_per_support: f64,
    layer_height_mm: Option<f64>,
) -> Vec<DetectedIsland> {
    let mut annotated = islands.to_vec();
    annotate_filter_flags(
        &mut annotated,
        &IslandFilterInput {
            support_tips,
            plate_z,
            supported_radius_mm: None,
            grounded_epsilon_mm: None,
            layer_height_mm,
        },
    );
    let mut counts = vec![0usize; annotated.len()];
    let z_tolerance = match layer_height_mm {
        Some(l) if l != 0.0 && !l.is_nan() => 2.0 * l,
        _ => 0.5,
    };
    for tip in support_tips {
        let mut this_tip: HashSet<usize> = HashSet::new();
        let actual_radius = tip.diameter_mm / 2.0 + 0.15;
        let actual_radius_sq = actual_radius * actual_radius;
        for cand in grid.query(tip.pos.x, tip.pos.y, actual_radius) {
            if this_tip.contains(&cand.island_index) {
                continue;
            }
            if (tip.pos.z - cand.z).abs() > z_tolerance {
                continue;
            }
            let (dx, dy) = (tip.pos.x - cand.x, tip.pos.y - cand.y);
            if dx * dx + dy * dy <= actual_radius_sq {
                this_tip.insert(cand.island_index);
            }
        }
        for idx in this_tip {
            counts[idx] += 1;
        }
    }
    for (island, &count) in annotated.iter_mut().zip(&counts) {
        let area = island.area_mm2.unwrap_or(0.0);
        island.support_count = Some(count as f64);
        island.supported = Some(count > 0);
        let required = js_max(1.0, (area / area_per_support).ceil());
        island.fully_supported = Some(count as f64 >= required);
    }
    annotated
}

/// The island panel's settings the derivation reads (the hook's state).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandPanelSettings {
    pub px_mm: f64,
    pub min_area_mm2: f64,
    pub consolidate_voxel: bool,
    pub consolidation_distance: f64,
    pub enable_contour_regions: bool,
    pub max_contour_regions: usize,
    pub area_per_support: f64,
    pub layer_height_mm: f64,
    pub plate_z: f64,
    pub filter_toggles: IslandFilterToggles,
}

impl IslandPanelSettings {
    /// The hook's defaults for a print at `layer_height_mm`.
    pub fn defaults(layer_height_mm: f64) -> Self {
        IslandPanelSettings {
            px_mm: 0.05,
            min_area_mm2: 0.02,
            consolidate_voxel: false,
            consolidation_distance: 0.2,
            enable_contour_regions: true,
            max_contour_regions: 20,
            area_per_support: 4.0,
            layer_height_mm,
            plate_z: 0.0,
            filter_toggles: super::filtering::DEFAULT_FILTER_TOGGLES,
        }
    }
}

/// What the hook hands on: the list auto-support runs on (`filteredIslands`),
/// the annotated set behind it, the classification statistics and the pose
/// verdict from the soup scan.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedIslands {
    pub filtered: Vec<DetectedIsland>,
    pub annotated: Vec<DetectedIsland>,
    pub stats: IntersectionStats,
    pub topple_coverage: bool,
    pub drag_total_mm3: f64,
}

/// The scan's islands as `useIslands` derives them. `combined` is the native
/// combined scan (its voxel islands and minima); `soup_scan` is the overhang
/// scan the frontend runs on its own world soup, whose regions replace the
/// combined scan's (DragonFruit's sideload path does the same, for triangle ids).
pub fn derive_islands(
    combined: &CombinedIslandScanResult,
    soup_scan: &OverhangScan,
    support_tips: &[TipInfo],
    settings: &IslandPanelSettings,
) -> DerivedIslands {
    let voxel: Vec<DetectedIsland> = combined
        .voxel_islands
        .iter()
        .filter(|v| ipc32(v.area_mm2) >= settings.min_area_mm2)
        .map(|v| DetectedIsland {
            id: v.id.clone(),
            source: IslandSource::Voxel,
            contact: ipc_vec3(v.contact),
            base_z: v.base_z,
            area_mm2: Some(ipc32(v.area_mm2)),
            layer_span: Some([f64::from(v.layer_span[0]), f64::from(v.layer_span[1])]),
            ..DetectedIsland::default()
        })
        .collect();
    let minima: Vec<DetectedIsland> = combined
        .minima_islands
        .iter()
        .enumerate()
        .map(|(i, m)| DetectedIsland {
            id: format!("m{i}"),
            source: IslandSource::Minima,
            contact: ipc_vec3(m.position),
            base_z: ipc32(m.position.z),
            vertex_index: Some(m.vertex_index as usize),
            seed_triangle_id: Some(m.seed_triangle_id as usize),
            ..DetectedIsland::default()
        })
        .collect();
    let overhang: Vec<DetectedIsland> = soup_scan
        .regions
        .iter()
        .enumerate()
        .map(|(i, r)| overhang_region_to_island(r, i))
        .collect();

    let opts = IntersectionOptions {
        xy_tolerance_mm: 0.5,
        z_band_mm: settings.layer_height_mm,
    };
    let proposed = if settings.consolidate_voxel {
        consolidate_voxel_islands(&voxel, settings.consolidation_distance, settings.px_mm)
    } else {
        voxel
    };
    let proposed_classified = classify_intersection(&proposed, &minima, opts);
    let contoured = if settings.enable_contour_regions {
        determine_contour_threshold(&proposed_classified.islands, settings.max_contour_regions)
    } else {
        HashSet::new()
    };
    let mut final_voxel: Vec<DetectedIsland> = Vec::new();
    for island in proposed {
        match &island.members {
            Some(members) if members.len() > 1 && !contoured.contains(&island.id) => {
                final_voxel.extend(members.iter().cloned());
            }
            _ => final_voxel.push(island),
        }
    }
    let classified = classify_intersection(&final_voxel, &minima, opts);
    let merged = merge_overhang_regions(classified.islands, &overhang);
    let grid = build_island_contact_grid(&merged);
    let annotated = annotate_and_count_supports(
        &merged,
        &grid,
        support_tips,
        settings.plate_z,
        settings.area_per_support,
        Some(settings.layer_height_mm),
    );
    let filtered = apply_filter(annotated.clone(), settings.filter_toggles);
    DerivedIslands {
        filtered,
        annotated,
        stats: classified.stats,
        topple_coverage: scan_needs_coverage(soup_scan),
        drag_total_mm3: soup_scan
            .stability
            .as_ref()
            .map_or(0.0, |s| ipc_null_as_zero(s.drag_moment_mm3)),
    }
}

//! The zone rules' candidates (SoulCrafted, not a DragonFruit file, #360):
//! every support a zone preset asks for, with the size and spacing of its
//! zone. See [`super::zone_rules`] for the zones.
//!
//! The candidates come back ordered by urgency (islands first, then their
//! reinforcement, overhangs and stabilization; lower first within a zone)
//! and already thinned: two candidates closer than half the coarser of their
//! two spacings are one, and the more urgent stays.

use std::collections::{HashMap, HashSet};

use super::grid_placement::{
    PERIMETER_CONTACT_INSET_MM, SurfaceSampler, build_boundary_points, face_normal_at,
};
use super::support_blockers::is_support_blocked_contact;
use super::types::{AutoSupportMesh, CandidatePoint, CandidateSource, js_compare};
use super::zone_rules::ZoneRules;
use crate::native::islands::body::BODY_CELL_MM;
use crate::three::math_utils::js_to_fixed;
use crate::three::{Ray, Vec3};
use crate::three_mesh_bvh::Side;
use crate::volume_analysis::islands::types::{DetectedIsland, IslandClass, IslandSource, Vec3Loop};
use crate::volume_analysis::islands::voxel_footprint::FootprintPoint;

/// Which zone a candidate serves, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Zone {
    Island,
    Reinforcement,
    Overhang,
    Stabilization,
}

/// What the zone pass reads.
pub struct ZoneInput<'a> {
    /// The islands to cover, overhang regions included.
    pub islands: &'a [DetectedIsland],
    /// The down-facing regions up to
    /// [`super::zone_rules::STABILIZATION_MAX_ANGLE_DEG`].
    pub steep: &'a [DetectedIsland],
    pub rules: &'a ZoneRules,
    pub layer_height_mm: f64,
    pub mesh: Option<AutoSupportMesh<'a>>,
}

struct Pending {
    candidate: CandidatePoint,
    zone: Zone,
    spacing_mm: f64,
}

/// The first surface an upward ray from below `(x, y)` meets between `lo`
/// and `hi`, with its face's normal.
fn surface_up(
    mesh: Option<AutoSupportMesh<'_>>,
    x: f64,
    y: f64,
    lo: f64,
    hi: f64,
) -> Option<(Vec3, Vec3)> {
    let m = mesh?;
    let from = lo - 1.0;
    let ray = Ray {
        origin: Vec3::new(x, y, from),
        direction: Vec3::new(0.0, 0.0, 1.0),
    };
    let hit = m
        .mesh
        .rays
        .intersect_object(&ray, Side::Double, 0.0, hi - from, false)
        .into_iter()
        .find(|h| h.point.z >= lo && h.point.z <= hi)?;
    let normal = face_normal_at(m.mesh.rays, hit.face_index)?;
    Some((hit.point, normal))
}

/// Points of a square lattice `spacing` apart over `cells` (centres of
/// [`BODY_CELL_MM`] cells), each inside a cell.
fn lattice_over_cells(cells: &[[f32; 2]], spacing: f64) -> Vec<(f64, f64)> {
    if cells.is_empty() || !(spacing > 0.0) {
        return Vec::new();
    }
    let key = |x: f64, y: f64| {
        (
            (x / BODY_CELL_MM).floor() as i64,
            (y / BODY_CELL_MM).floor() as i64,
        )
    };
    let set: HashSet<(i64, i64)> = cells
        .iter()
        .map(|c| key(f64::from(c[0]), f64::from(c[1])))
        .collect();
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for c in cells {
        let (x, y) = (f64::from(c[0]), f64::from(c[1]));
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    // Centred on the footprint, so a footprint narrower than the spacing
    // still gets its middle.
    let nx = ((max_x - min_x) / spacing).floor();
    let ny = ((max_y - min_y) / spacing).floor();
    let x0 = (min_x + max_x) / 2.0 - nx * spacing / 2.0;
    let y0 = (min_y + max_y) / 2.0 - ny * spacing / 2.0;
    let mut out = Vec::new();
    for i in 0..=(nx as i64) {
        for j in 0..=(ny as i64) {
            let x = x0 + i as f64 * spacing;
            let y = y0 + j as f64 * spacing;
            if set.contains(&key(x, y)) {
                out.push((x, y));
            }
        }
    }
    out
}

/// Surface points about `spacing` apart **on the surface** over a region:
/// the boundary ring and a lattice of a third of the spacing in plan (inset by
/// the contact radius), thinned so no two kept points are closer than the
/// spacing in 3D. A slope gets the density its surface asks for, not that
/// of its shadow on the plate. `(x, y, z, face)` per point.
fn region_points(
    island: &DetectedIsland,
    spacing: f64,
    mesh: Option<AutoSupportMesh<'_>>,
) -> Vec<(f64, f64, f64, Option<usize>)> {
    let dense = region_samples(island, spacing / 3.0, spacing, mesh);
    let min_d2 = (spacing * 0.9) * (spacing * 0.9);
    let cell = spacing;
    let key = |x: f64, y: f64, z: f64| {
        (
            (x / cell).floor() as i64,
            (y / cell).floor() as i64,
            (z / cell).floor() as i64,
        )
    };
    let mut kept: Vec<(f64, f64, f64, Option<usize>)> = Vec::new();
    let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    for p in dense {
        let (cx, cy, cz) = key(p.0, p.1, p.2);
        let mut close = false;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &k in grid.get(&(cx + dx, cy + dy, cz + dz)).into_iter().flatten() {
                        let q = kept[k];
                        let d2 = (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2);
                        if d2 < min_d2 {
                            close = true;
                            break 'search;
                        }
                    }
                }
            }
        }
        if !close {
            grid.entry((cx, cy, cz)).or_default().push(kept.len());
            kept.push(p);
        }
    }
    kept
}

/// The boundary ring (`ring_spacing` apart, first) and the plan lattice
/// (`spacing` apart) of a region, on its surface. Where the region folds over
/// itself in plan, the lattice takes every fold: each is an underside to hold,
/// and the lowest says nothing of the ones over it (#500).
fn region_samples(
    island: &DetectedIsland,
    spacing: f64,
    ring_spacing: f64,
    mesh: Option<AutoSupportMesh<'_>>,
) -> Vec<(f64, f64, f64, Option<usize>)> {
    let Some(voxels) = island.contact_voxels.as_ref().filter(|v| v.count > 0) else {
        return Vec::new();
    };
    let points = voxels.to_points();
    let mut sampler = SurfaceSampler::triangle(island, mesh.map(|m| m.mesh.rays))
        .unwrap_or_else(|| SurfaceSampler::voxel(&points, spacing.max(1.0), island.base_z));
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in &points {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    let inset = PERIMETER_CONTACT_INSET_MM;
    let span_x = (max_x - min_x - 2.0 * inset).max(0.0);
    let span_y = (max_y - min_y - 2.0 * inset).max(0.0);
    let nx = (span_x / spacing).floor();
    let ny = (span_y / spacing).floor();
    let x0 = (min_x + max_x) / 2.0 - nx * spacing / 2.0;
    let y0 = (min_y + max_y) / 2.0 - ny * spacing / 2.0;
    let mut out: Vec<(f64, f64, f64, Option<usize>)> = Vec::new();
    for i in 0..=(nx as i64) {
        for j in 0..=(ny as i64) {
            let x = x0 + i as f64 * spacing;
            let y = y0 + j as f64 * spacing;
            for s in sampler.all_at(x, y) {
                out.push((x, y, s.z, s.face_index));
            }
        }
    }
    let on_loops = island
        .perimeter_loops
        .as_deref()
        .map(|loops| ring_on_folds(loops, ring_spacing, &mut sampler))
        .filter(|ring| !ring.is_empty());
    let mut ring_points = on_loops.unwrap_or_else(|| {
        build_boundary_points(&points, ring_spacing, island.base_z)
            .into_iter()
            .map(|b| {
                let sampled = sampler.at(b.x, b.y);
                (
                    b.x,
                    b.y,
                    sampled.map_or(b.z, |s| s.z),
                    sampled.and_then(|s| s.face_index).or(b.face_index),
                )
            })
            .collect()
    });
    ring_points.extend(out);
    ring_points
}

/// How far from a perimeter loop, in height, the fold it runs on may be: the
/// loop is inset along the surface, so it stands a little off it.
const RING_FOLD_REACH_MM: f64 = 0.5;

/// Points `spacing` apart along a region's perimeter `loops` (inset, closed,
/// in space), each on the surface of the fold its loop runs on: of the
/// region's surfaces over the point, the one at the loop's height. A loop
/// around a part of the region that hangs over another part keeps its points
/// up there (#500).
fn ring_on_folds(
    loops: &[Vec3Loop],
    spacing: f64,
    sampler: &mut SurfaceSampler<'_>,
) -> Vec<(f64, f64, f64, Option<usize>)> {
    let mut out: Vec<(f64, f64, f64, Option<usize>)> = Vec::new();
    if !(spacing > 0.0) {
        return out;
    }
    let min_d2 = (spacing * 0.5) * (spacing * 0.5);
    let mut take = |x: f64, y: f64, z: f64, out: &mut Vec<(f64, f64, f64, Option<usize>)>| {
        let Some(hit) = sampler
            .all_at(x, y)
            .into_iter()
            .min_by(|a, b| (a.z - z).abs().total_cmp(&(b.z - z).abs()))
            .filter(|hit| (hit.z - z).abs() <= RING_FOLD_REACH_MM)
        else {
            return;
        };
        let close = out
            .iter()
            .any(|q| (x - q.0).powi(2) + (y - q.1).powi(2) + (hit.z - q.2).powi(2) < min_d2);
        if !close {
            out.push((x, y, hit.z, hit.face_index));
        }
    };
    for lp in loops.iter().filter(|lp| lp.len() >= 2) {
        // How far along the loop the next point stands from this corner.
        let mut next = 0.0;
        for i in 0..lp.len() {
            let (a, b) = (lp[i], lp[(i + 1) % lp.len()]);
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len < 1e-9 {
                continue;
            }
            while next <= len {
                let t = next / len;
                take(a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t, &mut out);
                next += spacing;
            }
            next -= len;
        }
    }
    out
}

fn island_source(island: &DetectedIsland) -> CandidateSource {
    if island.class == Some(IslandClass::Intersection) {
        return CandidateSource::Intersection;
    }
    match island.source {
        IslandSource::Voxel => CandidateSource::Voxel,
        IslandSource::Minima => CandidateSource::Minima,
        IslandSource::Overhang => CandidateSource::Overhang,
    }
}

/// Every candidate the rules ask for, thinned and ordered (see the module).
pub fn generate_zone_candidates(input: &ZoneInput<'_>) -> Vec<CandidatePoint> {
    let rules = input.rules;
    let lh = input.layer_height_mm.max(0.001);
    let mesh = input.mesh;
    let mut pending: Vec<Pending> = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();
    let mut push = |id: String,
                    pos: Vec3,
                    normal: Option<Vec3>,
                    source: CandidateSource,
                    zone: Zone,
                    size: usize,
                    spacing: f64,
                    area: f64,
                    grid: bool| {
        // A lattice point keeps the surface it was sampled on; anything else
        // (or a point whose face does not face down) is resolved at placement.
        let normal = normal.filter(|n| n.z < 0.0);
        // Two folds of a region over the same lattice point: the second
        // carries its height.
        let id = if ids.contains(&id) {
            format!("{id}-{}", js_to_fixed(pos.z, 2))
        } else {
            id
        };
        ids.insert(id.clone());
        pending.push(Pending {
            candidate: CandidatePoint {
                id,
                tip_pos: pos,
                tip_normal: normal.unwrap_or(Vec3::new(0.0, 0.0, -1.0)),
                model_id: String::new(),
                source,
                island_area_mm2: area,
                z_height: pos.z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: (grid && normal.is_some()).then_some(true),
                zone_size: Some(size),
            },
            zone,
            spacing_mm: spacing,
        });
    };

    let section_heights = rules.section_heights_mm();
    for island in input.islands {
        if island.source == IslandSource::Overhang {
            let rule = &rules.overhang;
            let area = island.area_mm2.unwrap_or(0.0);
            let Some(size) = rules.size_index(&rule.size).filter(|_| rule.enabled) else {
                continue;
            };
            if area < rule.min_area_mm2 {
                continue;
            }
            let points = region_points(island, rule.spacing_mm, mesh);
            if points.is_empty() {
                push(
                    format!("zo-{}", island.id),
                    island.contact,
                    island.surface_normal,
                    CandidateSource::Overhang,
                    Zone::Overhang,
                    size,
                    rule.spacing_mm,
                    area,
                    false,
                );
            }
            for (x, y, z, face) in points {
                let normal = match (mesh, face) {
                    (Some(m), Some(f)) => face_normal_at(m.mesh.rays, f),
                    _ => island.surface_normal,
                };
                push(
                    format!(
                        "zo-{}-{}-{}",
                        island.id,
                        js_to_fixed(x, 2),
                        js_to_fixed(y, 2)
                    ),
                    Vec3::new(x, y, z),
                    normal,
                    CandidateSource::Overhang,
                    Zone::Overhang,
                    size,
                    rule.spacing_mm,
                    rule.spacing_mm * rule.spacing_mm,
                    true,
                );
            }
            continue;
        }

        // An island: its first layer, then its reinforcement.
        let area = island.area_mm2.unwrap_or(0.0);
        let body = island.body.as_deref();
        let free = body.is_some_and(|b| !b.attached);
        let ignore = &rules.ignore_small_islands;
        if free
            && body.is_some_and(|b| ignore.ignores(f64::from(b.volume_mm3), f64::from(b.height_mm)))
        {
            continue;
        }
        let base_z = island.base_z;
        let source = island_source(island);
        if let Some(band) = rules.first_layer_band(area)
            && let Some(size) = rules.size_index(&band.size)
        {
            push(
                format!("zi-{}", island.id),
                island.contact,
                None,
                source,
                Zone::Island,
                size,
                band.spacing_mm,
                area,
                false,
            );
            let half = band.spacing_mm * 0.5;
            let lattice = body.map_or_else(Vec::new, |b| {
                lattice_over_cells(&b.first_layer_cells, band.spacing_mm)
            });
            for (k, (x, y)) in lattice.into_iter().enumerate() {
                let (dx, dy) = (x - island.contact.x, y - island.contact.y);
                if dx * dx + dy * dy < half * half {
                    continue;
                }
                let Some((pos, normal)) =
                    surface_up(mesh, x, y, base_z - 2.0 * lh, base_z + 2.0 * lh)
                else {
                    continue;
                };
                push(
                    format!("zi-{}-{k}", island.id),
                    pos,
                    Some(normal),
                    source,
                    Zone::Island,
                    size,
                    band.spacing_mm,
                    area,
                    false,
                );
            }
        }
        let Some(body) = body.filter(|_| free) else {
            continue;
        };
        let Some(band) = rules.reinforcement_band(f64::from(body.volume_mm3)) else {
            continue;
        };
        let Some(size) = rules.size_index(&band.size) else {
            continue;
        };
        let Some(cells) = section_heights
            .iter()
            .position(|h| *h == band.height_mm)
            .and_then(|k| body.sections.get(k))
            .filter(|c| !c.is_empty())
        else {
            continue;
        };
        let footprint: Vec<FootprintPoint> = cells
            .iter()
            .map(|c| FootprintPoint {
                x: f64::from(c[0]),
                y: f64::from(c[1]),
                z: None,
            })
            .collect();
        let ring = build_boundary_points(&footprint, band.spacing_mm, base_z);
        for (k, p) in ring.into_iter().enumerate() {
            let Some((pos, normal)) = surface_up(
                mesh,
                p.x,
                p.y,
                base_z + lh,
                base_z + band.height_mm + 2.0 * lh,
            ) else {
                continue;
            };
            if normal.z > -0.1 {
                continue;
            }
            push(
                format!("zr-{}-{k}", island.id),
                pos,
                Some(normal),
                CandidateSource::Reinforcement,
                Zone::Reinforcement,
                size,
                band.spacing_mm,
                band.spacing_mm * band.spacing_mm,
                false,
            );
        }
    }

    // Stabilization: the steep down-facing regions, outside the overhangs.
    let rule = &rules.stabilization;
    if let Some(size) = rules.size_index(&rule.size).filter(|_| rule.enabled) {
        let overhang_nz = -(rules.self_support_angle_deg().to_radians().cos());
        let overhangs_on = rules.overhang.enabled;
        for region in input.steep {
            if region.area_mm2.unwrap_or(0.0) < rule.min_area_mm2 {
                continue;
            }
            for (x, y, z, face) in region_points(region, rule.spacing_mm, mesh) {
                let normal = match (mesh, face) {
                    (Some(m), Some(f)) => face_normal_at(m.mesh.rays, f),
                    _ => region.surface_normal,
                };
                if overhangs_on && normal.is_some_and(|n| n.z < overhang_nz) {
                    continue;
                }
                push(
                    format!(
                        "zs-{}-{}-{}",
                        region.id,
                        js_to_fixed(x, 2),
                        js_to_fixed(y, 2)
                    ),
                    Vec3::new(x, y, z),
                    normal,
                    CandidateSource::Overhang,
                    Zone::Stabilization,
                    size,
                    rule.spacing_mm,
                    rule.spacing_mm * rule.spacing_mm,
                    true,
                );
            }
        }
    }

    // Painted blockers refuse a contact, as they do everywhere else.
    if let Some(m) = mesh {
        pending.retain(|p| {
            !is_support_blocked_contact(m.blocked, m.mesh.rays, m.side, p.candidate.tip_pos, None)
        });
    }
    let max_z = pending
        .iter()
        .fold(1.0f64, |m, p| m.max(p.candidate.z_height));
    for p in &mut pending {
        // Zone first, then lower first.
        let zone = 3.0 - p.zone as u8 as f64;
        p.candidate.priority = zone + (1.0 - p.candidate.z_height / max_z) * 0.5;
    }
    pending.sort_by(|a, b| js_compare(b.candidate.priority - a.candidate.priority));
    let anchors: HashSet<String> = input
        .islands
        .iter()
        .map(|island| format!("zi-{}", island.id))
        .collect();
    thin(pending, &anchors)
}

/// How much further than [`ZoneRules::dedup_radius_mm`] a candidate that is
/// not an island's first point stands off: the whole spacing the preset asks
/// for. The tips of the reference prints stand that far apart (#465).
const THINNING_SPREAD: f64 = 2.0;

/// Drops every candidate closer to a more urgent one than the coarser of their
/// spacings. `anchors`, the first point of each island, only give way at half
/// that: every island keeps its tip.
fn thin(pending: Vec<Pending>, anchors: &HashSet<String>) -> Vec<CandidatePoint> {
    let cell = pending
        .iter()
        .map(|p| ZoneRules::dedup_radius_mm(p.spacing_mm, p.spacing_mm) * THINNING_SPREAD)
        .fold(0.2f64, f64::max);
    let key = |v: Vec3| {
        (
            (v.x / cell).floor() as i64,
            (v.y / cell).floor() as i64,
            (v.z / cell).floor() as i64,
        )
    };
    let mut kept: Vec<Pending> = Vec::new();
    let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    for p in pending {
        let (cx, cy, cz) = key(p.candidate.tip_pos);
        let mut close = false;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(bucket) = grid.get(&(cx + dx, cy + dy, cz + dz)) else {
                        continue;
                    };
                    for &k in bucket {
                        let q = &kept[k];
                        let r = ZoneRules::dedup_radius_mm(p.spacing_mm, q.spacing_mm)
                            * if anchors.contains(&p.candidate.id) {
                                1.0
                            } else {
                                THINNING_SPREAD
                            };
                        let d = p.candidate.tip_pos.sub(q.candidate.tip_pos);
                        if d.length_sq() < r * r {
                            close = true;
                            break 'search;
                        }
                    }
                }
            }
        }
        if !close {
            grid.entry((cx, cy, cz)).or_default().push(kept.len());
            kept.push(p);
        }
    }
    kept.into_iter().map(|p| p.candidate).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lattice_stays_inside_its_cells() {
        // A 2 × 1 mm strip of cells.
        let mut cells = Vec::new();
        for i in 0..8 {
            for j in 0..4 {
                cells.push([(i as f32 + 0.5) * 0.25, (j as f32 + 0.5) * 0.25]);
            }
        }
        let points = lattice_over_cells(&cells, 0.8);
        assert_eq!(points.len(), 3, "{points:?}");
        for (x, y) in points {
            assert!((0.0..2.0).contains(&x) && (0.0..1.0).contains(&y));
        }
        // Narrower than the spacing: its middle.
        assert_eq!(lattice_over_cells(&cells, 5.0).len(), 1);
    }

    #[test]
    fn thinning_keeps_the_more_urgent_of_a_close_pair() {
        let candidate = |id: &str, x: f64| CandidatePoint {
            id: id.into(),
            tip_pos: Vec3::new(x, 0.0, 0.0),
            tip_normal: Vec3::new(0.0, 0.0, -1.0),
            model_id: String::new(),
            source: CandidateSource::Voxel,
            island_area_mm2: 0.0,
            z_height: 0.0,
            priority: 0.0,
            tip_diameter_mm: None,
            grid_point: None,
            zone_size: Some(0),
        };
        let pending = vec![
            Pending {
                candidate: candidate("island", 0.0),
                zone: Zone::Island,
                spacing_mm: 0.8,
            },
            Pending {
                candidate: candidate("overhang-near", 0.8),
                zone: Zone::Overhang,
                spacing_mm: 1.8,
            },
            Pending {
                candidate: candidate("overhang-far", 1.8),
                zone: Zone::Overhang,
                spacing_mm: 1.8,
            },
            Pending {
                candidate: candidate("island-next", 0.8 + 0.5),
                zone: Zone::Island,
                spacing_mm: 0.8,
            },
        ];
        let kept: Vec<String> = thin(pending, &HashSet::new())
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(kept, ["island", "overhang-far"]);
    }
}

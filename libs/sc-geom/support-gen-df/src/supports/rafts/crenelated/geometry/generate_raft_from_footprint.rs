//! DragonFruit `geometry/generateRaftFromFootprint.ts`: raft meshes from a
//! footprint polygon set. An untrimmed footprint (one ring, no holes) keeps
//! the convex generators; a trimmed one goes through the set-aware builders.
//! `build_raft_footprint_meshes` is the seam the exporter and slicer share.

use super::super::raft_types::{RaftBottomMode, RaftSettings, SupportBaseCircle};
use super::compute_raft_footprint::{compute_raft_footprint_polygons, raft_wall_base_height_mm};
use super::generate_chamfered_base::generate_chamfered_base;
use super::generate_crenelated_wall_manual::generate_crenelated_wall_manual;
use super::generate_perimeter_wall::generate_perimeter_wall;
use super::polygon_set_2d::PolygonWithHoles;
use super::small::{inset_convex_polygon, signed_area_2d};
use crate::clipper::ClipperError;
use crate::three::Vector2;
use crate::three::buffer_geometry::{BufferGeometry, merge_geometries};
use crate::three::math_utils::{js_hypot, js_max, js_min, js_tan};
use crate::three::shape_utils::triangulate_shape;

fn has_rings(polys: &[PolygonWithHoles]) -> bool {
    polys
        .iter()
        .any(|p| p.outer.len() >= 3 || p.holes.iter().any(|h| h.len() >= 3))
}

fn clamp_angle(angle: f64) -> f64 {
    js_min(
        90.0,
        js_max(45.0, if angle.is_finite() { angle } else { 90.0 }),
    )
}

/// Move a ring toward the solid (holes grow) with the shift clamped to four
/// times the distance.
fn inset_ring_for_solid_side(ring: &[Vector2], is_hole: bool, distance_mm: f64) -> Vec<Vector2> {
    let inset = inset_convex_polygon(ring, if is_hole { -distance_mm } else { distance_mm });
    if inset.len() != ring.len() {
        return ring.to_vec();
    }
    let max_shift = js_max(distance_mm * 4.0, 1e-4);
    inset
        .iter()
        .enumerate()
        .map(|(index, &point)| {
            let dx = point.x - ring[index].x;
            let dy = point.y - ring[index].y;
            let length = js_hypot(&[dx, dy]);
            if length <= max_shift || length == 0.0 {
                return point;
            }
            let scale = max_shift / length;
            Vector2::new(ring[index].x + dx * scale, ring[index].y + dy * scale)
        })
        .collect()
}

/// `generateChamferedBaseFromPolygons(polys, { thickness, chamferAngle })`.
/// The cap triangulation trims a closing duplicate off the caller's rings in
/// place, as `ShapeUtils.triangulateShape` does in DragonFruit.
pub fn generate_chamfered_base_from_polygons(
    polys: &mut [PolygonWithHoles],
    thickness: f64,
    chamfer_angle: f64,
) -> BufferGeometry {
    let thickness = js_max(0.0, thickness);
    if thickness == 0.0 || !has_rings(polys) {
        return BufferGeometry::default();
    }
    let inset =
        thickness * js_tan((std::f64::consts::PI / 180.0) * (90.0 - clamp_angle(chamfer_angle)));

    let mut positions: Vec<f64> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut layouts: Vec<(usize, Vec<usize>)> = Vec::new();

    let mut vertex_count = 0;
    for poly in polys.iter() {
        let outer_start = vertex_count;
        for p in &poly.outer {
            positions.extend([p.x, p.y, thickness]);
        }
        vertex_count += poly.outer.len();
        let mut hole_starts = Vec::new();
        for hole in &poly.holes {
            hole_starts.push(vertex_count);
            for p in hole {
                positions.extend([p.x, p.y, thickness]);
            }
            vertex_count += hole.len();
        }
        layouts.push((outer_start, hole_starts));
    }

    let bottom_base = vertex_count;
    for poly in polys.iter() {
        for p in inset_ring_for_solid_side(&poly.outer, false, inset) {
            positions.extend([p.x, p.y, 0.0]);
        }
        for hole in &poly.holes {
            for p in inset_ring_for_solid_side(hole, true, inset) {
                positions.extend([p.x, p.y, 0.0]);
            }
        }
    }

    for (poly, (outer_start, _)) in polys.iter().zip(&layouts) {
        let mut ring_start = *outer_start;
        let entries =
            std::iter::once((&poly.outer, false)).chain(poly.holes.iter().map(|h| (h, true)));
        for (ring, is_hole) in entries {
            let top_start = ring_start;
            let bottom_start = bottom_base + top_start;
            let flip = is_hole == (signed_area_2d(ring) > 0.0);
            for i in 0..ring.len() {
                let next = (i + 1) % ring.len();
                let (a, b) = ((top_start + i) as u32, (top_start + next) as u32);
                let (c, d) = ((bottom_start + next) as u32, (bottom_start + i) as u32);
                if flip {
                    indices.extend([a, b, c, a, c, d]);
                } else {
                    indices.extend([a, c, b, a, d, c]);
                }
            }
            ring_start += ring.len();
        }
    }

    for (poly, (outer_start, hole_starts)) in polys.iter_mut().zip(&layouts) {
        let mut contour_to_vertex: Vec<usize> =
            (0..poly.outer.len()).map(|i| outer_start + i).collect();
        for (h, hole) in poly.holes.iter().enumerate() {
            contour_to_vertex.extend((0..hole.len()).map(|i| hole_starts[h] + i));
        }
        for tri in triangulate_shape(&mut poly.outer, &mut poly.holes) {
            let Some(mapped) = tri
                .iter()
                .map(|&i| contour_to_vertex.get(i).map(|&v| v as u32))
                .collect::<Option<Vec<u32>>>()
            else {
                continue;
            };
            indices.extend([mapped[0], mapped[1], mapped[2]]);
            let bb = bottom_base as u32;
            indices.extend([bb + mapped[0], bb + mapped[2], bb + mapped[1]]);
        }
    }

    if !positions.iter().all(|v| v.is_finite()) {
        return BufferGeometry::default();
    }
    let mut geometry = BufferGeometry::default();
    geometry.set_position(&positions);
    geometry.index = Some(indices);
    geometry.compute_vertex_normals();
    geometry
}

/// `mergeWallParts`: every part non-indexed with only position and normal,
/// then `mergeGeometries`.
fn merge_wall_parts(parts: &[BufferGeometry]) -> Option<BufferGeometry> {
    let prepared: Vec<BufferGeometry> = parts
        .iter()
        .map(|g| {
            let mut n = g.to_non_indexed();
            n.uv = None;
            if n.normal.is_none() {
                n.compute_vertex_normals();
            }
            n
        })
        .collect();
    if prepared.is_empty() {
        return None;
    }
    merge_geometries(&prepared)
}

/// `generateWallFromPolygons`: a perimeter wall around every outer ring; holes
/// are skipped.
pub fn generate_wall_from_polygons(
    polys: &[PolygonWithHoles],
    settings: &RaftSettings,
    thickness: f64,
) -> Option<BufferGeometry> {
    let wall_height = js_max(0.0, settings.wall_height);
    let wall_thickness = js_max(0.0, settings.wall_thickness);
    if wall_height == 0.0 || wall_thickness == 0.0 {
        return None;
    }
    let use_crenels = settings.crenulation_spacing > 0.0 && settings.crenulation_gap_width > 0.0;
    let mut parts = Vec::new();
    for poly in polys {
        if poly.outer.len() < 3 {
            continue;
        }
        let mesh = if use_crenels {
            generate_crenelated_wall_manual(
                &poly.outer,
                wall_height,
                wall_thickness,
                settings.crenulation_gap_width,
                thickness,
                settings.chamfer_angle,
            )
        } else {
            generate_perimeter_wall(&poly.outer, wall_height, wall_thickness, thickness)
        };
        if mesh.vertex_count() > 0 {
            parts.push(mesh);
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut merged = merge_wall_parts(&parts)?;
    merged.compute_vertex_normals();
    Some(merged)
}

pub fn is_untrimmed_footprint(polys: &[PolygonWithHoles]) -> bool {
    polys.len() == 1 && polys[0].holes.is_empty()
}

/// `RaftFootprintMeshes`.
#[derive(Debug, Clone, Default)]
pub struct RaftFootprintMeshes {
    pub footprint: Vec<PolygonWithHoles>,
    pub base_mesh: Option<BufferGeometry>,
    pub wall_mesh: Option<BufferGeometry>,
}

/// `buildRaftFootprintMeshes({ circles, raft, clearance })`.
pub fn build_raft_footprint_meshes(
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    clearance: Option<&[PolygonWithHoles]>,
) -> Result<RaftFootprintMeshes, ClipperError> {
    let mut footprint = compute_raft_footprint_polygons(circles, raft, clearance, None)?;
    if footprint.is_empty() {
        return Ok(RaftFootprintMeshes::default());
    }
    let untrimmed = is_untrimmed_footprint(&footprint);
    let use_crenels = raft.crenulation_spacing > 0.0 && raft.crenulation_gap_width > 0.0;
    let wall_base_height = raft_wall_base_height_mm(raft);

    let base_mesh = if raft.bottom_mode == RaftBottomMode::Solid {
        Some(if untrimmed {
            generate_chamfered_base(&footprint[0].outer, raft)
        } else {
            generate_chamfered_base_from_polygons(
                &mut footprint,
                raft.thickness,
                raft.chamfer_angle,
            )
        })
    } else {
        None
    };

    let wall_mesh = if raft.wall_enabled {
        if untrimmed {
            let profile = &footprint[0].outer;
            Some(if use_crenels {
                generate_crenelated_wall_manual(
                    profile,
                    raft.wall_height,
                    raft.wall_thickness,
                    raft.crenulation_gap_width,
                    wall_base_height,
                    raft.chamfer_angle,
                )
            } else {
                generate_perimeter_wall(
                    profile,
                    raft.wall_height,
                    raft.wall_thickness,
                    wall_base_height,
                )
            })
        } else {
            generate_wall_from_polygons(&footprint, raft, wall_base_height)
        }
    } else {
        None
    };

    Ok(RaftFootprintMeshes {
        footprint,
        base_mesh,
        wall_mesh,
    })
}

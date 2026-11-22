//! A solid raft's meshes from its outline: the plate and the wall. An
//! outline that is one ring without holes keeps the convex builders; any
//! other is built ring by ring, its holes' sides sloped outward.

use super::super::raft_types::{RaftBottomMode, RaftSettings, SupportBaseCircle};
use super::compute_raft_footprint::{
    compute_raft_footprint_polygons, inflate_model_plate_clearance, raft_dynamic_margin_mm,
    raft_wall_base_height_mm,
};
use super::polygon_set_2d::{PolygonError, PolygonWithHoles, difference_polygon_sets};
use super::raft_body::{
    generate_chamfered_base, generate_crenelated_wall_manual, generate_perimeter_wall,
    inset_convex_polygon, signed_area_2d,
};
use super::raft_shapes;
use bread_support_geom::geom::Vector2;
use bread_support_geom::geom::buffer_geometry::{BufferGeometry, merge_geometries};
use bread_support_geom::geom::shape_utils::triangulate_shape;

/// `ring` moved `distance` toward the solid (a hole grows), each point at
/// most four distances from where it was; the ring as it is when the move
/// changes its point count.
fn toward_solid(ring: &[Vector2], hole: bool, distance: f64) -> Vec<Vector2> {
    let moved = inset_convex_polygon(ring, if hole { -distance } else { distance });
    if moved.len() != ring.len() {
        return ring.to_vec();
    }
    let reach = (distance * 4.0).max(1e-4);
    moved
        .iter()
        .zip(ring)
        .map(|(&m, &r)| {
            let (dx, dy) = (m.x - r.x, m.y - r.y);
            let length = (dx * dx + dy * dy).sqrt();
            if length <= reach || length == 0.0 {
                m
            } else {
                Vector2::new(r.x + dx * reach / length, r.y + dy * reach / length)
            }
        })
        .collect()
}

/// A plate over the polygons: top rings at `thickness`, bottom rings moved
/// in by the chamfer, sides between them, both caps triangulated from the
/// top rings. The triangulation trims a closing duplicate off the rings in
/// place.
pub fn generate_chamfered_base_from_polygons(
    polys: &mut [PolygonWithHoles],
    thickness: f64,
    chamfer_angle: f64,
) -> BufferGeometry {
    let thickness = thickness.max(0.0);
    let any_ring = polys
        .iter()
        .any(|p| p.outer.len() >= 3 || p.holes.iter().any(|h| h.len() >= 3));
    if thickness == 0.0 || !any_ring {
        return BufferGeometry::default();
    }
    let angle = if chamfer_angle.is_finite() {
        chamfer_angle
    } else {
        90.0
    };
    let run = thickness * (90.0 - angle.clamp(45.0, 90.0)).to_radians().tan();
    // Every ring with whether it is a hole, polygon by polygon.
    let rings = |polys: &[PolygonWithHoles]| -> Vec<(Vec<Vector2>, bool)> {
        polys
            .iter()
            .flat_map(|p| {
                std::iter::once((p.outer.clone(), false))
                    .chain(p.holes.iter().map(|h| (h.clone(), true)))
            })
            .collect()
    };
    let all = rings(polys);
    let top_count: usize = all.iter().map(|r| r.0.len()).sum();
    let mut positions: Vec<f64> = all
        .iter()
        .flat_map(|(ring, _)| ring.iter().flat_map(|p| [p.x, p.y, thickness]))
        .collect();
    positions.extend(
        all.iter()
            .flat_map(|(ring, hole)| toward_solid(ring, *hole, run))
            .flat_map(|p| [p.x, p.y, 0.0]),
    );
    let mut indices: Vec<u32> = Vec::new();
    let mut first = 0usize;
    for (ring, hole) in &all {
        // Sides facing out of the solid: a ring turning the solid's way.
        let flip = *hole == (signed_area_2d(ring) > 0.0);
        let n = ring.len();
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b) = ((first + i) as u32, (first + j) as u32);
            let (c, d) = (
                (top_count + first + j) as u32,
                (top_count + first + i) as u32,
            );
            if flip {
                indices.extend([a, b, c, a, c, d]);
            } else {
                indices.extend([a, c, b, a, d, c]);
            }
        }
        first += n;
    }
    let mut first = 0usize;
    for poly in polys.iter_mut() {
        let mut number: Vec<usize> = (0..poly.outer.len()).map(|i| first + i).collect();
        let mut at = first + poly.outer.len();
        for hole in &poly.holes {
            number.extend((0..hole.len()).map(|i| at + i));
            at += hole.len();
        }
        first = at;
        for t in triangulate_shape(&mut poly.outer, &mut poly.holes) {
            let Some(corners) = t
                .iter()
                .map(|&i| number.get(i).map(|&v| v as u32))
                .collect::<Option<Vec<u32>>>()
            else {
                continue;
            };
            let below = top_count as u32;
            indices.extend([corners[0], corners[1], corners[2]]);
            indices.extend([below + corners[0], below + corners[2], below + corners[1]]);
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

/// A wall around each polygon's outline (holes get none), plain or gapped,
/// standing on `base_z`.
pub fn generate_wall_from_polygons(
    polys: &[PolygonWithHoles],
    settings: &RaftSettings,
    base_z: f64,
) -> Option<BufferGeometry> {
    let (height, thickness) = (
        settings.wall_height.max(0.0),
        settings.wall_thickness.max(0.0),
    );
    if height == 0.0 || thickness == 0.0 {
        return None;
    }
    let gapped = settings.crenulation_spacing > 0.0 && settings.crenulation_gap_width > 0.0;
    let walls: Vec<BufferGeometry> = polys
        .iter()
        .filter(|p| p.outer.len() >= 3)
        .map(|p| {
            if gapped {
                generate_crenelated_wall_manual(
                    &p.outer,
                    height,
                    thickness,
                    settings.crenulation_gap_width,
                    base_z,
                    settings.chamfer_angle,
                )
            } else {
                generate_perimeter_wall(&p.outer, height, thickness, base_z)
            }
        })
        .filter(|g| g.vertex_count() > 0)
        .map(|g| {
            let mut flat = g.to_non_indexed();
            flat.uv = None;
            if flat.normal.is_none() {
                flat.compute_vertex_normals();
            }
            flat
        })
        .collect();
    if walls.is_empty() {
        return None;
    }
    let mut wall = merge_geometries(&walls)?;
    wall.compute_vertex_normals();
    Some(wall)
}

/// One ring without holes: the convex builders draw it.
pub fn is_untrimmed_footprint(polys: &[PolygonWithHoles]) -> bool {
    polys.len() == 1 && polys[0].holes.is_empty()
}

/// A raft's outline and its meshes.
#[derive(Debug, Clone, Default)]
pub struct RaftFootprintMeshes {
    pub footprint: Vec<PolygonWithHoles>,
    pub base_mesh: Option<BufferGeometry>,
    pub wall_mesh: Option<BufferGeometry>,
}

/// The raft over `circles`: its outline (reshaped into pads, tabs or holes
/// when asked, #138 #99), its plate (solid rafts) and its wall.
pub fn build_raft_footprint_meshes(
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    clearance: Option<&[PolygonWithHoles]>,
) -> Result<RaftFootprintMeshes, PolygonError> {
    let mut outline = compute_raft_footprint_polygons(circles, raft, clearance, None)?;
    if outline.is_empty() {
        return Ok(RaftFootprintMeshes::default());
    }
    let solid = raft.bottom_mode == RaftBottomMode::Solid;
    let reshaped = solid && (raft.pads || raft.peel_tabs || raft.perforation_mm > 0.0);
    if reshaped {
        let margin = raft_dynamic_margin_mm(raft);
        if raft.pads {
            outline = raft_shapes::pads(circles, margin)?;
            if let Some(clear) = clearance.filter(|c| !c.is_empty()) {
                outline =
                    difference_polygon_sets(&outline, &inflate_model_plate_clearance(clear)?)?;
            }
        }
        if raft.peel_tabs {
            outline = raft_shapes::with_peel_tabs(&outline)?;
        }
        if raft.perforation_mm > 0.0 {
            outline = raft_shapes::perforated(&outline, circles, raft.perforation_mm, margin)?;
        }
        if outline.is_empty() {
            return Ok(RaftFootprintMeshes::default());
        }
    }
    let convex = !reshaped && is_untrimmed_footprint(&outline);
    let base_z = raft_wall_base_height_mm(raft);
    let base_mesh = solid.then(|| {
        if convex {
            generate_chamfered_base(&outline[0].outer, raft)
        } else {
            generate_chamfered_base_from_polygons(&mut outline, raft.thickness, raft.chamfer_angle)
        }
    });
    let wall_mesh = if !raft.wall_enabled {
        None
    } else if convex {
        let ring = &outline[0].outer;
        Some(
            if raft.crenulation_spacing > 0.0 && raft.crenulation_gap_width > 0.0 {
                generate_crenelated_wall_manual(
                    ring,
                    raft.wall_height,
                    raft.wall_thickness,
                    raft.crenulation_gap_width,
                    base_z,
                    raft.chamfer_angle,
                )
            } else {
                generate_perimeter_wall(ring, raft.wall_height, raft.wall_thickness, base_z)
            },
        )
    } else {
        generate_wall_from_polygons(&outline, raft, base_z)
    };
    Ok(RaftFootprintMeshes {
        footprint: outline,
        base_mesh,
        wall_mesh,
    })
}

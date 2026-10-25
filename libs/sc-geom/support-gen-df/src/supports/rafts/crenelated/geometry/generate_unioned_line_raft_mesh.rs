//! DragonFruit `geometry/generateUnionedLineRaftMesh.ts`: the line raft's beam
//! network unioned into one solid, and the filter that drops beams a model on
//! the plate is in the way of.

use super::generate_raft_from_footprint::generate_chamfered_base_from_polygons;
use super::polygon_set_2d::{
    PolygonWithHoles, polygon_set_contains, polygon_set_to_shapes, ring_to_polygon,
    segment_distance_mm, union_polygon_sets,
};
use super::small::inset_convex_polygon;
use crate::clipper::ClipperError;
use crate::three::Vector2;
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::extrude_geometry::{ExtrudeOptions, extrude_geometry};
use crate::three::math_utils::js_max;

/// Beam rectangle around `a -> b`, extended by half a width at each end.
fn build_beam_ring(a: Vector2, b: Vector2, width_mm: f64) -> Vec<Vector2> {
    let half_width = js_max(0.001, width_mm / 2.0);
    let dir = b.sub(a);
    let length = dir.length();
    if !length.is_finite() || length < 1e-6 {
        return Vec::new();
    }
    let dir = dir.multiply_scalar(1.0 / length);
    let start = a.add_scaled_vector(dir, -half_width);
    let end = b.add_scaled_vector(dir, half_width);
    let normal = Vector2::new(-dir.y, dir.x);
    vec![
        start.add_scaled_vector(normal, half_width),
        end.add_scaled_vector(normal, half_width),
        end.add_scaled_vector(normal, -half_width),
        start.add_scaled_vector(normal, -half_width),
    ]
}

/// `filterLineRaftEdges(edges, clearance, beamWidthMm)`.
pub fn filter_line_raft_edges(
    edges: &[(Vector2, Vector2)],
    clearance: Option<&[PolygonWithHoles]>,
    beam_width_mm: f64,
) -> Vec<(Vector2, Vector2)> {
    let Some(clearance) = clearance.filter(|c| !c.is_empty()) else {
        return edges.to_vec();
    };
    let margin = js_max(0.0, beam_width_mm) / 2.0;
    edges
        .iter()
        .copied()
        .filter(|&(a, b)| {
            let (mid_x, mid_y) = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            if polygon_set_contains(clearance, a.x, a.y)
                || polygon_set_contains(clearance, b.x, b.y)
                || polygon_set_contains(clearance, mid_x, mid_y)
            {
                return false;
            }
            for poly in clearance {
                for ring in std::iter::once(&poly.outer).chain(&poly.holes) {
                    for i in 0..ring.len() {
                        let (c, d) = (ring[i], ring[(i + 1) % ring.len()]);
                        if segment_distance_mm(a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y) <= margin {
                            return false;
                        }
                    }
                }
            }
            true
        })
        .collect()
}

/// `generateUnionedLineRaftMesh(edges, { widthMm, heightMm, borderProfile,
/// chamferAngleDeg })`.
pub fn generate_unioned_line_raft_mesh(
    edges: &[(Vector2, Vector2)],
    width_mm: f64,
    height_mm: f64,
    border_profile: Option<&[Vector2]>,
    chamfer_angle_deg: Option<f64>,
) -> Result<BufferGeometry, ClipperError> {
    let width = js_max(0.001, width_mm);
    let height = js_max(0.001, height_mm);
    let mut subjects: Vec<PolygonWithHoles> = Vec::new();
    for &(a, b) in edges {
        let ring = build_beam_ring(a, b, width);
        if ring.len() >= 3 {
            subjects.push(ring_to_polygon(ring));
        }
    }
    if let Some(border) = border_profile.filter(|b| b.len() >= 3) {
        let inner = inset_convex_polygon(border, width);
        if inner.len() >= 3 {
            subjects.push(PolygonWithHoles {
                outer: border.to_vec(),
                holes: vec![inner],
            });
        }
    }
    if subjects.is_empty() {
        return Ok(BufferGeometry::default());
    }
    let mut polys = union_polygon_sets(&subjects)?;
    if polys.is_empty() {
        return Ok(BufferGeometry::default());
    }
    let extrude = |polys: &[PolygonWithHoles]| {
        let mut g = extrude_geometry(
            &polygon_set_to_shapes(polys),
            ExtrudeOptions {
                depth: height,
                steps: 1,
            },
        );
        g.compute_vertex_normals();
        g
    };
    let use_chamfer = chamfer_angle_deg.is_some_and(|a| a.is_finite() && a < 89.999);
    let Some(angle) = chamfer_angle_deg.filter(|_| use_chamfer) else {
        return Ok(extrude(&polys));
    };
    // `generateChamferedBaseFromPolygons` hands the set to earcut, which may
    // trim its rings in place before the fallback extrusion reads them.
    let chamfered = generate_chamfered_base_from_polygons(&mut polys, height, angle);
    if chamfered.vertex_count() > 0 {
        return Ok(chamfered);
    }
    Ok(extrude(&polys))
}

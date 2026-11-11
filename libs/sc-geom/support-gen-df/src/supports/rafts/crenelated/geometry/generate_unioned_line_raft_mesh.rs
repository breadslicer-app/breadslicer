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

/// How many sides the round joint at each end of a beam has.
const JOINT_SIDES: usize = 24;

/// Beam rectangle from `a` to `b`, ending at them: the round joint at each
/// end ([`build_joint_ring`]) closes it. Square ends stretched past the ends,
/// as DragonFruit draws them, stuck out as little corners where several beams
/// meet (SoulCrafted, #518).
fn build_beam_ring(a: Vector2, b: Vector2, width_mm: f64) -> Vec<Vector2> {
    let half_width = js_max(0.001, width_mm / 2.0);
    let dir = b.sub(a);
    let length = dir.length();
    if !length.is_finite() || length < 1e-6 {
        return Vec::new();
    }
    let dir = dir.multiply_scalar(1.0 / length);
    let (start, end) = (a, b);
    let normal = Vector2::new(-dir.y, dir.x);
    vec![
        start.add_scaled_vector(normal, half_width),
        end.add_scaled_vector(normal, half_width),
        end.add_scaled_vector(normal, -half_width),
        start.add_scaled_vector(normal, -half_width),
    ]
}

/// A disk as wide as a beam round `at`, turning the way a beam ring does.
fn build_joint_ring(at: Vector2, width_mm: f64) -> Vec<Vector2> {
    let r = js_max(0.001, width_mm / 2.0);
    (0..JOINT_SIDES)
        .map(|k| {
            let a = -std::f64::consts::TAU * k as f64 / JOINT_SIDES as f64;
            Vector2::new(at.x + r * a.cos(), at.y + r * a.sin())
        })
        .collect()
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
    let mut joints: Vec<Vector2> = Vec::new();
    for &(a, b) in edges {
        let ring = build_beam_ring(a, b, width);
        if ring.len() >= 3 {
            subjects.push(ring_to_polygon(ring));
            for end in [a, b] {
                if !joints.iter().any(|j| j.sub(end).length() < 1e-6) {
                    joints.push(end);
                }
            }
        }
    }
    for at in joints {
        subjects.push(ring_to_polygon(build_joint_ring(at, width)));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// #518: where beams meet, nothing sticks out past half a line's width
    /// from them: the joints are round, not the beams' square ends.
    #[test]
    fn beams_meet_in_round_joints_with_no_corners_out() {
        let n = |x: f64, y: f64| Vector2::new(x, y);
        // Three beams meeting at the origin at uneven angles, and a fourth
        // from one of their far ends.
        let edges = [
            (n(0.0, 0.0), n(10.0, 0.0)),
            (n(0.0, 0.0), n(-4.0, 9.0)),
            (n(0.0, 0.0), n(-7.0, -6.0)),
            (n(10.0, 0.0), n(14.0, 8.0)),
        ];
        let width = 1.5;
        let mesh = generate_unioned_line_raft_mesh(&edges, width, 0.6, None, None).unwrap();
        let points = mesh.position.as_ref().expect("a mesh");
        assert!(!points.is_empty());
        let seg = |p: Vector2, a: Vector2, b: Vector2| {
            let d = b.sub(a);
            let t = (((p.x - a.x) * d.x + (p.y - a.y) * d.y) / (d.x * d.x + d.y * d.y))
                .clamp(0.0, 1.0);
            (p.x - a.x - d.x * t).hypot(p.y - a.y - d.y * t)
        };
        let mut farthest: f64 = 0.0;
        for p in points.as_chunks::<3>().0 {
            let at = n(f64::from(p[0]), f64::from(p[1]));
            let near = edges
                .iter()
                .map(|&(a, b)| seg(at, a, b))
                .fold(f64::MAX, f64::min);
            farthest = farthest.max(near);
        }
        assert!(farthest <= width / 2.0 + 1e-3, "{farthest}");
    }
}

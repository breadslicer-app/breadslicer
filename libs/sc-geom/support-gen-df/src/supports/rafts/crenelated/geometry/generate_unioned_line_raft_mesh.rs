//! DragonFruit `geometry/generateUnionedLineRaftMesh.ts`: the line raft's beam
//! network unioned into one solid, and the filter that drops beams a model on
//! the plate is in the way of.

use std::collections::HashMap;

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

/// A point this close to a cap's edge lies on it, mm.
const ON_EDGE_MM: f64 = 1e-5;

/// SoulCrafted: splits the flat faces (the caps) of `g` where a point of the
/// mesh lies on one of their edges. Beams on one line (a grid lays them so)
/// leave holes in the union with sides on that line, and earcut runs one cap
/// edge past their corners while the walls stop there: a T in the mesh, open
/// edges the slicer refuses. A split face becomes a fan about its centre, so
/// every edge on the line meets one of the walls'. The geometry comes out
/// without an index, uvs or groups; only its positions are read.
fn split_at_t_junctions(g: &mut BufferGeometry) {
    let Some(position) = g.position.take() else {
        return;
    };
    let corners: Vec<[f32; 3]> = match &g.index {
        Some(index) => index
            .iter()
            .map(|&i| {
                let i = i as usize * 3;
                [position[i], position[i + 1], position[i + 2]]
            })
            .collect(),
        None => position
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| [p[0], p[1], p[2]])
            .collect(),
    };
    // Every point once, by cell and by height.
    let cell = |v: f64| (v / CELL_MM).floor() as i64;
    const CELL_MM: f64 = 1.0;
    let mut grid: HashMap<(i64, i64, u32), Vec<[f32; 3]>> = HashMap::new();
    for p in &corners {
        let points = grid
            .entry((cell(f64::from(p[0])), cell(f64::from(p[1])), p[2].to_bits()))
            .or_default();
        if !points.contains(p) {
            points.push(*p);
        }
    }
    let on_edge = |a: [f32; 3], b: [f32; 3]| -> Vec<(f64, [f32; 3])> {
        let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
        let (dx, dy) = (f64::from(b[0]) - ax, f64::from(b[1]) - ay);
        let len2 = dx * dx + dy * dy;
        if len2 <= 0.0 {
            return Vec::new();
        }
        let len = len2.sqrt();
        let mut found = Vec::new();
        let (x0, x1) = (cell(ax.min(ax + dx)), cell(ax.max(ax + dx)));
        let (y0, y1) = (cell(ay.min(ay + dy)), cell(ay.max(ay + dy)));
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                let Some(points) = grid.get(&(cx, cy, a[2].to_bits())) else {
                    continue;
                };
                for p in points {
                    if *p == a || *p == b {
                        continue;
                    }
                    let (px, py) = (f64::from(p[0]) - ax, f64::from(p[1]) - ay);
                    let t = (px * dx + py * dy) / len2;
                    if t > 0.0 && t < 1.0 && (px * dy - py * dx).abs() / len <= ON_EDGE_MM {
                        found.push((t, *p));
                    }
                }
            }
        }
        found.sort_by(|x, y| x.0.total_cmp(&y.0));
        found
    };
    let mut out: Vec<f32> = Vec::with_capacity(position.len());
    for t in corners.as_chunks::<3>().0 {
        let [a, b, c] = *t;
        let flat = a[2] == b[2] && b[2] == c[2];
        let rims: Vec<Vec<(f64, [f32; 3])>> = if flat {
            vec![on_edge(a, b), on_edge(b, c), on_edge(c, a)]
        } else {
            Vec::new()
        };
        if rims.iter().all(Vec::is_empty) {
            for p in [a, b, c] {
                out.extend_from_slice(&p);
            }
            continue;
        }
        let mut ring = Vec::new();
        for (corner, rim) in [a, b, c].into_iter().zip(&rims) {
            ring.push(corner);
            ring.extend(rim.iter().map(|&(_, p)| p));
        }
        let centre = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0);
        for k in 0..ring.len() {
            for p in [centre, ring[k], ring[(k + 1) % ring.len()]] {
                out.extend_from_slice(&p);
            }
        }
    }
    g.position = Some(out);
    g.index = None;
    g.normal = None;
    g.uv = None;
    g.groups.clear();
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
        split_at_t_junctions(&mut g);
        g.compute_vertex_normals();
        g
    };
    let use_chamfer = chamfer_angle_deg.is_some_and(|a| a.is_finite() && a < 89.999);
    let Some(angle) = chamfer_angle_deg.filter(|_| use_chamfer) else {
        return Ok(extrude(&polys));
    };
    // `generateChamferedBaseFromPolygons` hands the set to earcut, which may
    // trim its rings in place before the fallback extrusion reads them.
    let mut chamfered = generate_chamfered_base_from_polygons(&mut polys, height, angle);
    if chamfered.vertex_count() > 0 {
        split_at_t_junctions(&mut chamfered);
        chamfered.compute_vertex_normals();
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
            let t =
                (((p.x - a.x) * d.x + (p.y - a.y) * d.y) / (d.x * d.x + d.y * d.y)).clamp(0.0, 1.0);
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

    /// Edges of `points` (three floats per corner) used by one face only,
    /// with corners told apart by their position.
    fn open_edges(points: &[f32]) -> usize {
        let key = |p: &[f32; 3]| p.map(f32::to_bits);
        let mut count: HashMap<([u32; 3], [u32; 3]), i32> = HashMap::new();
        for t in points.as_chunks::<9>().0 {
            let corners = [[t[0], t[1], t[2]], [t[3], t[4], t[5]], [t[6], t[7], t[8]]];
            for k in 0..3 {
                let (a, b) = (key(&corners[k]), key(&corners[(k + 1) % 3]));
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        count.values().filter(|&&n| n == 1).count()
    }

    /// #530: beams on a 7 mm grid with diagonals leave holes whose sides lie
    /// on the same line; the raft still closes, so the slicer takes it.
    #[test]
    fn beams_on_one_line_still_make_a_closed_raft() {
        let n = |x: f64, y: f64| Vector2::new(x, y);
        let mut edges = Vec::new();
        for i in 0..4 {
            for j in 0..3 {
                let (x, y) = (7.0 * f64::from(i), 7.0 * f64::from(j));
                if i < 3 {
                    edges.push((n(x, y), n(x + 7.0, y)));
                }
                if j < 2 {
                    edges.push((n(x, y), n(x, y + 7.0)));
                }
                if i < 3 && j < 2 && (i + j) % 2 == 0 {
                    edges.push((n(x, y), n(x + 7.0, y + 7.0)));
                }
            }
        }
        for chamfer in [None, Some(45.0)] {
            let mesh = generate_unioned_line_raft_mesh(&edges, 1.5, 0.6, None, chamfer).unwrap();
            let points = mesh.position.as_ref().expect("a mesh");
            assert!(!points.is_empty());
            assert_eq!(open_edges(points), 0, "chamfer {chamfer:?}");
        }
    }
}

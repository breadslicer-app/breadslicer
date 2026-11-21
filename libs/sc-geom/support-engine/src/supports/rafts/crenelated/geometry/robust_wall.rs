//! SoulCrafted's closed raft geometry (not a DragonFruit file).
//!
//! DragonFruit's own rasteriser fills whatever the raft meshes enclose; a
//! contour slicer needs them closed and manifold, which two of them are not:
//!
//! - The wall. DragonFruit builds each wall ring by moving the outline
//!   inward with `insetConvexPolygon`, which assumes a convex outline. Once a
//!   model on the plate trims the footprint the outline is concave: the inset
//!   ring self-intersects, and both the crenelated wall and its
//!   perimeter-wall fallback come out tangled. Here the wall is the outline
//!   minus the outline offset inward by the wall thickness (Clipper, which
//!   takes any simple polygon), with DragonFruit's crenel gaps cut out of
//!   that band as rectangles, then extruded. It is vertical (DragonFruit's
//!   chamfer flare at the top is not reproduced).
//! - The trimmed base. Its side walls run through every ring point, but
//!   earcut drops exactly collinear points from the caps, among them points
//!   a hole's bridge leaves collinear, and the caps then span the side walls'
//!   edges there (T-junctions). Here the rings lose their collinear points
//!   first, and a cap triangle spanning a vertex is split at it.

use super::generate_raft_from_footprint::generate_chamfered_base_from_polygons;
use super::polygon_set_2d::{
    PolygonWithHoles, difference_polygon_sets, offset_polygon_set, polygon_set_to_shapes,
};
use crate::clipper::ClipperError;
use crate::supports::rafts::crenelated::raft_types::RaftSettings;
use crate::three::Vector2;
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::extrude_geometry::{ExtrudeOptions, extrude_geometry};
use crate::three::math_utils::{clamp, js_acos, js_max, js_min};

/// `generateCrenelatedWallManual`'s straight-edge test and gap placement: one
/// gap centred on every edge longer than 5 mm that turns less than 15° from
/// the previous one, inset by a 1 mm margin from both ends.
fn crenel_notches(outer: &[Vector2], gap_width: f64, wall_thickness: f64) -> Vec<PolygonWithHoles> {
    let n = outer.len();
    let mut notches = Vec::new();
    if gap_width <= 0.0 {
        return notches;
    }
    for i in 0..n {
        let (a, b) = (outer[i], outer[(i + 1) % n]);
        let edge = b.sub(a);
        let len = edge.length();
        if !len.is_finite() || len <= 5.0 {
            continue;
        }
        let dir = edge.normalize();
        let prev = a.sub(outer[(i + n - 1) % n]).normalize();
        let ang = js_acos(clamp(prev.dot(dir), -1.0, 1.0)).to_degrees();
        if ang > 15.0 {
            continue;
        }
        let margin = 1.0;
        let center = margin + js_max(0.0, len - 2.0 * margin) / 2.0;
        let (s, e) = (
            js_max(0.0, center - gap_width / 2.0),
            js_min(len, center + gap_width / 2.0),
        );
        // Across the wall: out past the outline, in past the inner ring.
        let normal = Vector2::new(dir.y, -dir.x);
        let (p0, p1) = (a.add_scaled_vector(dir, s), a.add_scaled_vector(dir, e));
        let (out, inn) = (wall_thickness, -2.0 * wall_thickness);
        notches.push(PolygonWithHoles {
            outer: vec![
                p0.add_scaled_vector(normal, inn),
                p1.add_scaled_vector(normal, inn),
                p1.add_scaled_vector(normal, out),
                p0.add_scaled_vector(normal, out),
            ],
            holes: Vec::new(),
        });
    }
    notches
}

/// A wall around every outer ring of `polys` (holes skipped, as
/// DragonFruit's `generateWallFromPolygons` does), standing on `base_z`.
/// `None` when the wall is off or nothing is left of it.
pub fn robust_wall_from_polygons(
    polys: &[PolygonWithHoles],
    raft: &RaftSettings,
    base_z: f64,
) -> Result<Option<BufferGeometry>, ClipperError> {
    let wall_height = js_max(0.0, raft.wall_height);
    let wall_thickness = js_max(0.0, raft.wall_thickness);
    if wall_height == 0.0 || wall_thickness == 0.0 {
        return Ok(None);
    }
    let crenels = raft.crenulation_spacing > 0.0 && raft.crenulation_gap_width > 0.0;
    let mut band: Vec<PolygonWithHoles> = Vec::new();
    for poly in polys {
        if poly.outer.len() < 3 {
            continue;
        }
        let ring = [PolygonWithHoles {
            outer: poly.outer.clone(),
            holes: Vec::new(),
        }];
        let inner = offset_polygon_set(&ring, -wall_thickness)?;
        let mut piece = difference_polygon_sets(&ring, &inner)?;
        if crenels {
            let notches = crenel_notches(&poly.outer, raft.crenulation_gap_width, wall_thickness);
            if !notches.is_empty() {
                piece = difference_polygon_sets(&piece, &notches)?;
            }
        }
        band.extend(piece);
    }
    let shapes = polygon_set_to_shapes(&band);
    if shapes.is_empty() {
        return Ok(None);
    }
    let mut geom = extrude_geometry(
        &shapes,
        ExtrudeOptions {
            depth: wall_height,
            steps: 1,
        },
    );
    geom.uv = None;
    geom.compute_vertex_normals();
    geom.translate(0.0, 0.0, js_max(0.0, base_z));
    Ok((geom.vertex_count() > 0).then_some(geom))
}

/// `ring` without the points earcut's `filterPoints` would drop: repeats of
/// the next point and points exactly on the line through their neighbours.
fn without_collinear_points(ring: &[Vector2]) -> Vec<Vector2> {
    let mut pts = ring.to_vec();
    loop {
        let n = pts.len();
        if n < 3 {
            return pts;
        }
        let drop = (0..n).find(|&i| {
            let (p, q, r) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let repeat = q.x == r.x && q.y == r.y;
            let area = (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y);
            repeat || area == 0.0
        });
        match drop {
            Some(i) => {
                pts.remove(i);
            }
            None => return pts,
        }
    }
}

/// DragonFruit's trimmed-footprint base (`generateChamferedBaseFromPolygons`)
/// on rings that earcut keeps whole, so caps and side walls share every edge.
pub fn robust_chamfered_base(polys: &[PolygonWithHoles], raft: &RaftSettings) -> BufferGeometry {
    let mut clean: Vec<PolygonWithHoles> = polys
        .iter()
        .map(|p| PolygonWithHoles {
            outer: without_collinear_points(&p.outer),
            holes: p
                .holes
                .iter()
                .map(|h| without_collinear_points(h))
                .filter(|h| h.len() >= 3)
                .collect(),
        })
        .filter(|p| p.outer.len() >= 3)
        .collect();
    let mut geom =
        generate_chamfered_base_from_polygons(&mut clean, raft.thickness, raft.chamfer_angle);
    split_t_junctions(&mut geom);
    geom
}

/// Splits every triangle whose edge, shared by no other triangle, passes
/// through other vertices of the mesh: a fan from the opposite corner through
/// those vertices, so the neighbours' edges are matched one for one.
fn split_t_junctions(geom: &mut BufferGeometry) {
    use std::collections::HashMap;
    let (Some(pos), Some(index)) = (geom.position.as_ref(), geom.index.as_mut()) else {
        return;
    };
    let p = |i: u32| {
        let i = i as usize * 3;
        [pos[i] as f64, pos[i + 1] as f64, pos[i + 2] as f64]
    };
    let vertex_count = pos.len() / 3;
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in index.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    // The vertices strictly inside segment a-b, ordered from a.
    let on_segment = |a: u32, b: u32| -> Vec<u32> {
        let (pa, pb) = (p(a), p(b));
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        if len2 <= 0.0 {
            return Vec::new();
        }
        let mut hits: Vec<(f64, u32)> = (0..vertex_count as u32)
            .filter(|&v| v != a && v != b)
            .filter_map(|v| {
                let pv = p(v);
                let ap = [pv[0] - pa[0], pv[1] - pa[1], pv[2] - pa[2]];
                let t = (ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2;
                let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t, ap[2] - ab[2] * t];
                let off2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                (t > 1e-6 && t < 1.0 - 1e-6 && off2 < 1e-12 * len2.max(1.0)).then_some((t, v))
            })
            .collect();
        hits.sort_by(|x, y| x.0.total_cmp(&y.0));
        hits.dedup_by(|x, y| p(x.1) == p(y.1));
        hits.into_iter().map(|(_, v)| v).collect()
    };
    let mut out: Vec<u32> = Vec::with_capacity(index.len());
    for t in index.as_chunks::<3>().0 {
        let split = (0..3).find_map(|k| {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if uses[&(a.min(b), a.max(b))] != 1 {
                return None;
            }
            let inside = on_segment(a, b);
            (!inside.is_empty()).then_some((k, inside))
        });
        match split {
            Some((k, inside)) => {
                let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                let chain: Vec<u32> = std::iter::once(a).chain(inside).chain([b]).collect();
                for w in chain.windows(2) {
                    out.extend([w[0], w[1], c]);
                }
            }
            None => out.extend_from_slice(t),
        }
    }
    *index = out;
}

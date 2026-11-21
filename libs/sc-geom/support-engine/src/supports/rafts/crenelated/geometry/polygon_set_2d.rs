//! Polygon sets in world-XY millimetres: union, difference and offset (on
//! `i_overlay`), containment and the clearance edge grid.
//!
//! A `PolygonWithHoles` keeps its outer counter-clockwise and its holes
//! clockwise; offsets are positive-outward. The booleans run on 64-bit
//! integers in `i_overlay`, exact on the inputs' grid.

use std::collections::HashMap;
use std::f64::consts::PI;

use i_float::adapter::FloatPointAdapter;
use i_float::float::rect::FloatRect;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::ShapeType;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::Solver;
use i_overlay::float::overlay::{FloatOverlay, OverlayOptions};
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

use super::small::signed_area_2d;
use crate::geom::Vector2;
use crate::geom::math_utils::{js_hypot, js_max, js_min};
use crate::geom::shape::{Path as ShapePath, Shape};

/// A failure of a polygon operation. The booleans and offsets cannot fail;
/// the type stays so the raft code keeps its fallbacks for a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolygonError {}

impl std::fmt::Display for PolygonError {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {}
    }
}

/// A miter join up to twice the offset distance, a bevel past it: the
/// corners at least 60 degrees wide keep their point.
const MITER_MIN_ANGLE: f64 = PI / 3.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolygonWithHoles {
    pub outer: Vec<Vector2>,
    pub holes: Vec<Vec<Vector2>>,
}

pub fn ring_to_polygon(ring: Vec<Vector2>) -> PolygonWithHoles {
    PolygonWithHoles {
        outer: ring,
        holes: Vec::new(),
    }
}

/// Copy of `ring` wound so its signed area is positive when `ccw`.
pub fn orient_ring(ring: &[Vector2], ccw: bool) -> Vec<Vector2> {
    let mut copy = ring.to_vec();
    if (signed_area_2d(&copy) > 0.0) != ccw {
        copy.reverse();
    }
    copy
}

type Contour = Vec<[f64; 2]>;

fn contour_of(ring: &[Vector2]) -> Contour {
    ring.iter().map(|p| [p.x, p.y]).collect()
}

/// The set's rings, the outers counter-clockwise and the holes clockwise.
fn contours_of(polys: &[PolygonWithHoles]) -> Vec<Contour> {
    let mut contours = Vec::new();
    for poly in polys {
        if poly.outer.len() >= 3 {
            contours.push(contour_of(&orient_ring(&poly.outer, true)));
        }
        for hole in &poly.holes {
            if hole.len() >= 3 {
                contours.push(contour_of(&orient_ring(hole, false)));
            }
        }
    }
    contours
}

/// `i_overlay` shapes (outer, then holes) as polygon sets.
fn polygons_of(shapes: Vec<Vec<Contour>>) -> Vec<PolygonWithHoles> {
    let ring = |c: Contour| c.into_iter().map(|[x, y]| Vector2::new(x, y)).collect();
    shapes
        .into_iter()
        .filter_map(|shape| {
            let mut rings = shape.into_iter();
            let outer: Vec<Vector2> = ring(rings.next()?);
            (outer.len() >= 3).then(|| PolygonWithHoles {
                outer,
                holes: rings
                    .map(ring)
                    .filter(|h: &Vec<Vector2>| h.len() >= 3)
                    .collect(),
            })
        })
        .collect()
}

/// Steps per millimetre the booleans snap to: micrometre precision. On a
/// fixed grid, points a hair apart become one point and a point a hair off a
/// line falls on it, so a union leaves no micron-long slivers.
const STEPS_PER_MM: f64 = 1000.0;

/// `subject` `rule` `clip`, non-zero filled, as OGC-valid shapes: rings that
/// never cross, holes inside their outer.
fn overlay(subject: &[Contour], clip: &[Contour], rule: OverlayRule) -> Vec<PolygonWithHoles> {
    let points = subject.iter().chain(clip).flatten();
    let Ok(Some(rect)) = FloatRect::with_iter(points) else {
        return Vec::new();
    };
    let adapter = FloatPointAdapter::<[f64; 2], i64>::with_scale(rect, STEPS_PER_MM);
    let capacity = subject.iter().chain(clip).map(Vec::len).sum();
    let shapes =
        FloatOverlay::new_custom(adapter, OverlayOptions::ogc(), Solver::default(), capacity)
            .unsafe_add_source(subject, ShapeType::Subject)
            .unsafe_add_source(clip, ShapeType::Clip)
            .overlay(rule, FillRule::NonZero);
    polygons_of(shapes)
}

fn union_of(contours: &[Contour]) -> Vec<PolygonWithHoles> {
    if contours.is_empty() {
        return Vec::new();
    }
    overlay(contours, &[], OverlayRule::Union)
}

/// Union of polygon sets, keeping holes as holes.
pub fn union_polygon_sets(
    sets: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, PolygonError> {
    Ok(union_of(&contours_of(sets)))
}

/// `subject` minus `clip`.
pub fn difference_polygon_sets(
    subject: &[PolygonWithHoles],
    clip: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, PolygonError> {
    let subject = contours_of(subject);
    if subject.is_empty() {
        return Ok(Vec::new());
    }
    let clip = contours_of(clip);
    if clip.is_empty() {
        return Ok(union_of(&subject));
    }
    Ok(overlay(&subject, &clip, OverlayRule::Difference))
}

/// Offset a polygon set by `delta_mm` (positive grows the solid), with miter
/// corners where they are at least 60 degrees wide and bevels past that.
pub fn offset_polygon_set(
    polys: &[PolygonWithHoles],
    delta_mm: f64,
) -> Result<Vec<PolygonWithHoles>, PolygonError> {
    let merged = union_of(&contours_of(polys));
    if merged.is_empty() {
        return Ok(Vec::new());
    }
    if delta_mm == 0.0 {
        return Ok(merged);
    }
    let shapes: Vec<Vec<Contour>> = merged
        .iter()
        .map(|p| {
            std::iter::once(contour_of(&p.outer))
                .chain(p.holes.iter().map(|h| contour_of(h)))
                .collect()
        })
        .collect();
    let style = OutlineStyle::new(delta_mm).line_join(LineJoin::Miter(MITER_MIN_ANGLE));
    let grown: Vec<Contour> = shapes
        .outline_as::<i64>(&style)
        .into_iter()
        .flatten()
        .collect();
    Ok(union_of(&grown))
}

/// Distance from a point to a segment, in mm.
pub fn point_to_segment_mm(x: f64, y: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq <= 1e-12 {
        0.0
    } else {
        js_max(
            0.0,
            js_min(1.0, ((x - ax) * dx + (y - ay) * dy) / length_sq),
        )
    };
    js_hypot(&[x - (ax + dx * t), y - (ay + dy * t)])
}

/// Ray-cast `inside` over one ring, toggling the caller's flag.
fn ring_toggles(ring: &[Vector2], x: f64, y: f64, inside: &mut bool) {
    let n = ring.len();
    if n == 0 {
        return;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > y) != (b.y > y) && x < ((b.x - a.x) * (y - a.y)) / (b.y - a.y) + a.x {
            *inside = !*inside;
        }
        j = i;
    }
}

/// Ray-cast containment over a polygon set, holes subtracted.
pub fn polygon_set_contains(polys: &[PolygonWithHoles], x: f64, y: f64) -> bool {
    for poly in polys {
        let mut inside = false;
        ring_toggles(&poly.outer, x, y, &mut inside);
        for hole in &poly.holes {
            ring_toggles(hole, x, y, &mut inside);
        }
        if inside {
            return true;
        }
    }
    false
}

/// A uniform grid over a polygon set's edges.
#[derive(Debug, Clone)]
pub struct ClearanceEdgeGrid {
    pub cell_size: f64,
    pub min_x: f64,
    pub min_y: f64,
    /// `cy * 100000 + cx` to the edges' offsets into `edges`.
    pub cells: HashMap<i64, Vec<usize>>,
    /// `[ax, ay, bx, by]` per edge.
    pub edges: Vec<f64>,
}

/// How far a query's box grows, so an edge a micron away is still found.
pub const EDGE_QUERY_EPS_MM: f64 = 0.001;

fn cell_of(value: f64, min: f64, cell_size: f64) -> i64 {
    js_max(0.0, ((value - min) / cell_size).floor()) as i64
}

pub fn build_clearance_edge_grid(
    clearance: &[PolygonWithHoles],
    cell_size: f64,
) -> Option<ClearanceEdgeGrid> {
    let mut edges: Vec<f64> = Vec::new();
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    for poly in clearance {
        for ring in std::iter::once(&poly.outer).chain(&poly.holes) {
            for i in 0..ring.len() {
                let a = ring[i];
                let b = ring[(i + 1) % ring.len()];
                edges.extend([a.x, a.y, b.x, b.y]);
                if a.x < min_x {
                    min_x = a.x;
                }
                if a.y < min_y {
                    min_y = a.y;
                }
            }
        }
    }
    if edges.is_empty() {
        return None;
    }
    let mut cells: HashMap<i64, Vec<usize>> = HashMap::new();
    let mut e = 0;
    while e < edges.len() {
        let x0 = js_min(edges[e], edges[e + 2]);
        let x1 = js_max(edges[e], edges[e + 2]);
        let y0 = js_min(edges[e + 1], edges[e + 3]);
        let y1 = js_max(edges[e + 1], edges[e + 3]);
        let (cx0, cx1) = (cell_of(x0, min_x, cell_size), cell_of(x1, min_x, cell_size));
        let (cy0, cy1) = (cell_of(y0, min_y, cell_size), cell_of(y1, min_y, cell_size));
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                cells.entry(cy * 100000 + cx).or_default().push(e);
            }
        }
        e += 4;
    }
    Some(ClearanceEdgeGrid {
        cell_size,
        min_x,
        min_y,
        cells,
        edges,
    })
}

/// Whether any edge in the cells the query segment's box covers lies within
/// `epsilon` of it.
pub fn grid_segment_blocked(
    grid: &ClearanceEdgeGrid,
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    epsilon: f64,
) -> bool {
    let cx0 = cell_of(js_min(ax, bx) - epsilon, grid.min_x, grid.cell_size);
    let cx1 = cell_of(js_max(ax, bx) + epsilon, grid.min_x, grid.cell_size);
    let cy0 = cell_of(js_min(ay, by) - epsilon, grid.min_y, grid.cell_size);
    let cy1 = cell_of(js_max(ay, by) + epsilon, grid.min_y, grid.cell_size);
    for cy in cy0..=cy1 {
        for cx in cx0..=cx1 {
            let Some(bucket) = grid.cells.get(&(cy * 100000 + cx)) else {
                continue;
            };
            for &e in bucket {
                let ed = &grid.edges[e..e + 4];
                if segment_distance_mm(ax, ay, bx, by, ed[0], ed[1], ed[2], ed[3]) <= epsilon {
                    return true;
                }
            }
        }
    }
    false
}

/// Closest approach of two segments, in mm.
#[allow(clippy::too_many_arguments)]
pub fn segment_distance_mm(
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    cx: f64,
    cy: f64,
    dx: f64,
    dy: f64,
) -> f64 {
    let d1 = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    let d2 = (bx - ax) * (dy - ay) - (by - ay) * (dx - ax);
    let d3 = (dx - cx) * (ay - cy) - (dy - cy) * (ax - cx);
    let d4 = (dx - cx) * (by - cy) - (dy - cy) * (bx - cx);
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) {
        return 0.0;
    }
    js_min(
        js_min(
            point_to_segment_mm(ax, ay, cx, cy, dx, dy),
            point_to_segment_mm(bx, by, cx, cy, dx, dy),
        ),
        js_min(
            point_to_segment_mm(cx, cy, ax, ay, bx, by),
            point_to_segment_mm(dx, dy, ax, ay, bx, by),
        ),
    )
}

/// `THREE.Shape` list (with holes) so `ExtrudeGeometry` can consume the set.
pub fn polygon_set_to_shapes(polys: &[PolygonWithHoles]) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for poly in polys {
        if poly.outer.len() < 3 {
            continue;
        }
        let mut shape = Shape::new(Some(&poly.outer));
        for hole in &poly.holes {
            if hole.len() < 3 {
                continue;
            }
            let mut path = ShapePath::new(Some(hole));
            path.close_path();
            shape.holes.push(path);
        }
        shapes.push(shape);
    }
    shapes
}

/// Net area of the set in mm² (holes subtracted).
pub fn polygon_set_area_mm2(polys: &[PolygonWithHoles]) -> f64 {
    let mut area = 0.0;
    for poly in polys {
        area += signed_area_2d(&poly.outer).abs();
        for hole in &poly.holes {
            area -= signed_area_2d(hole).abs();
        }
    }
    area
}

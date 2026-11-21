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

use super::raft_body::signed_area_2d;
use crate::geom::Vector2;
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

/// The distance from `(x, y)` to the segment `a b` (mm).
pub fn point_to_segment_mm(x: f64, y: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (ux, uy) = (bx - ax, by - ay);
    let squared = ux * ux + uy * uy;
    let t = if squared <= 1e-12 {
        0.0
    } else {
        (((x - ax) * ux + (y - ay) * uy) / squared).clamp(0.0, 1.0)
    };
    let (dx, dy) = (x - (ax + ux * t), y - (ay + uy * t));
    (dx * dx + dy * dy).sqrt()
}

/// Whether `(x, y)` is inside the ring, by the crossings of a ray toward
/// `+x`.
fn ring_holds(ring: &[Vector2], x: f64, y: f64) -> bool {
    let n = ring.len();
    (0..n)
        .filter(|&i| {
            let (a, b) = (ring[i], ring[(i + n - 1) % n]);
            (a.y > y) != (b.y > y) && x < (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x
        })
        .count()
        % 2
        == 1
}

/// Whether `(x, y)` is inside one of the polygons (its holes left out).
pub fn polygon_set_contains(polys: &[PolygonWithHoles], x: f64, y: f64) -> bool {
    polys.iter().any(|poly| {
        std::iter::once(&poly.outer)
            .chain(&poly.holes)
            .filter(|ring| ring_holds(ring, x, y))
            .count()
            % 2
            == 1
    })
}

/// The edges of a polygon set in a grid of square cells, for asking which
/// edges lie near a segment.
#[derive(Debug, Clone)]
pub struct ClearanceEdgeGrid {
    pub cell_size: f64,
    pub min_x: f64,
    pub min_y: f64,
    /// Each cell's edges (as offsets into `edges`), by `row · 100000 + column`.
    pub cells: HashMap<i64, Vec<usize>>,
    /// `[ax, ay, bx, by]` per edge.
    pub edges: Vec<f64>,
}

/// How far a query's box grows, so an edge a micron away is still found.
pub const EDGE_QUERY_EPS_MM: f64 = 0.001;

fn cell_of(value: f64, min: f64, cell_size: f64) -> i64 {
    ((value - min) / cell_size).floor().max(0.0) as i64
}

/// Every edge of the set in each cell its box covers; none without edges.
pub fn build_clearance_edge_grid(
    clearance: &[PolygonWithHoles],
    cell_size: f64,
) -> Option<ClearanceEdgeGrid> {
    let edges: Vec<f64> = clearance
        .iter()
        .flat_map(|poly| std::iter::once(&poly.outer).chain(&poly.holes))
        .flat_map(|ring| {
            (0..ring.len()).flat_map(move |i| {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                [a.x, a.y, b.x, b.y]
            })
        })
        .collect();
    if edges.is_empty() {
        return None;
    }
    // The grid starts at the edges' first corners' lowest x and y.
    let min_x = edges.chunks_exact(4).map(|e| e[0]).fold(f64::INFINITY, f64::min);
    let min_y = edges.chunks_exact(4).map(|e| e[1]).fold(f64::INFINITY, f64::min);
    let mut cells: HashMap<i64, Vec<usize>> = HashMap::new();
    for (k, e) in edges.chunks_exact(4).enumerate() {
        let columns = cell_of(e[0].min(e[2]), min_x, cell_size)..=cell_of(e[0].max(e[2]), min_x, cell_size);
        for row in cell_of(e[1].min(e[3]), min_y, cell_size)..=cell_of(e[1].max(e[3]), min_y, cell_size) {
            for column in columns.clone() {
                cells.entry(row * 100000 + column).or_default().push(k * 4);
            }
        }
    }
    Some(ClearanceEdgeGrid {
        cell_size,
        min_x,
        min_y,
        cells,
        edges,
    })
}

/// Whether an edge in the cells around segment `a b` comes within `epsilon`
/// of it.
pub fn grid_segment_blocked(
    grid: &ClearanceEdgeGrid,
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    epsilon: f64,
) -> bool {
    let columns = cell_of(ax.min(bx) - epsilon, grid.min_x, grid.cell_size)
        ..=cell_of(ax.max(bx) + epsilon, grid.min_x, grid.cell_size);
    let rows = cell_of(ay.min(by) - epsilon, grid.min_y, grid.cell_size)
        ..=cell_of(ay.max(by) + epsilon, grid.min_y, grid.cell_size);
    rows.flat_map(|row| columns.clone().map(move |column| row * 100000 + column))
        .filter_map(|cell| grid.cells.get(&cell))
        .flatten()
        .any(|&k| {
            let e = &grid.edges[k..k + 4];
            segment_distance_mm(ax, ay, bx, by, e[0], e[1], e[2], e[3]) <= epsilon
        })
}

/// How close segments `a b` and `c d` come (mm): 0 when they cross.
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
    let side = |px: f64, py: f64, qx: f64, qy: f64, rx: f64, ry: f64| (qx - px) * (ry - py) - (qy - py) * (rx - px) > 0.0;
    let crosses = side(ax, ay, bx, by, cx, cy) != side(ax, ay, bx, by, dx, dy)
        && side(cx, cy, dx, dy, ax, ay) != side(cx, cy, dx, dy, bx, by);
    if crosses {
        return 0.0;
    }
    point_to_segment_mm(ax, ay, cx, cy, dx, dy)
        .min(point_to_segment_mm(bx, by, cx, cy, dx, dy))
        .min(point_to_segment_mm(cx, cy, ax, ay, bx, by).min(point_to_segment_mm(dx, dy, ax, ay, bx, by)))
}

/// The set as shapes to extrude: each polygon with its holes, rings of
/// fewer than three points left out.
pub fn polygon_set_to_shapes(polys: &[PolygonWithHoles]) -> Vec<Shape> {
    polys
        .iter()
        .filter(|poly| poly.outer.len() >= 3)
        .map(|poly| {
            let mut shape = Shape::new(Some(&poly.outer));
            shape.holes.extend(
                poly.holes
                    .iter()
                    .filter(|hole| hole.len() >= 3)
                    .map(|hole| ShapePath::new(Some(hole))),
            );
            shape
        })
        .collect()
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

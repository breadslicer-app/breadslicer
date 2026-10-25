//! DragonFruit `geometry/polygonSet2d.ts`: polygon-set union, difference and
//! offset on Clipper (`crate::clipper`, the clipper-lib port), the PolyTree
//! flattening, containment and the clearance edge grid.
//!
//! Conventions as DragonFruit's: rings are world-XY millimetres; a
//! `PolygonWithHoles` keeps its outer counter-clockwise and its holes
//! clockwise; offsets are positive-outward. Clipper runs at 1000 steps per mm.
//!
//! A Clipper exception (the JavaScript throws, the export aborts) is an `Err`.

use std::collections::HashMap;

use super::small::signed_area_2d;
use crate::clipper::{
    ClipType, Clipper, ClipperError, ClipperOffset, EndType, IntPoint, JoinType, Path,
    PolyFillType, PolyTree, PolyType,
};
use crate::three::Vector2;
use crate::three::math_utils::{js_hypot, js_max, js_min, js_round};
use crate::three::shape::{Path as ShapePath, Shape};

/// 1000 steps per millimetre: micrometre precision.
pub const CLIPPER_SCALE: f64 = 1000.0;

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

pub fn to_int_point(p: Vector2) -> IntPoint {
    IntPoint::new(js_round(p.x * CLIPPER_SCALE), js_round(p.y * CLIPPER_SCALE))
}

pub fn to_int_path(ring: &[Vector2]) -> Path {
    ring.iter().map(|&p| to_int_point(p)).collect()
}

pub fn from_int_path(path: &[IntPoint]) -> Vec<Vector2> {
    path.iter()
        .map(|p| Vector2::new(p.x / CLIPPER_SCALE, p.y / CLIPPER_SCALE))
        .collect()
}

/// Copy of `ring` wound so its signed area is positive when `ccw`.
pub fn orient_ring(ring: &[Vector2], ccw: bool) -> Vec<Vector2> {
    let mut copy = ring.to_vec();
    if (signed_area_2d(&copy) > 0.0) != ccw {
        copy.reverse();
    }
    copy
}

/// `runUnion(paths, strictlySimple)`: NonZero union to a PolyTree, flattened.
fn run_union(paths: &[Path], strictly_simple: bool) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let usable: Vec<Path> = paths.iter().filter(|p| p.len() >= 3).cloned().collect();
    if usable.is_empty() {
        return Ok(Vec::new());
    }
    let mut c = Clipper::new();
    c.strictly_simple = strictly_simple;
    c.add_paths(&usable, PolyType::Subject, true)?;
    let mut tree = PolyTree::default();
    c.execute_tree(
        ClipType::Union,
        &mut tree,
        PolyFillType::NonZero,
        PolyFillType::NonZero,
    )?;
    Ok(polygons_from_poly_tree(&tree))
}

fn polygon_set_paths(polys: &[PolygonWithHoles]) -> Vec<Path> {
    let mut paths = Vec::new();
    for poly in polys {
        if poly.outer.len() >= 3 {
            paths.push(to_int_path(&orient_ring(&poly.outer, true)));
        }
        for hole in &poly.holes {
            if hole.len() >= 3 {
                paths.push(to_int_path(&orient_ring(hole, false)));
            }
        }
    }
    paths
}

/// Union of polygon sets, keeping holes as holes.
pub fn union_polygon_sets(
    sets: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    run_union(&polygon_set_paths(sets), true)
}

pub fn union_rings(rings: &[Vec<Vector2>]) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let paths: Vec<Path> = rings
        .iter()
        .filter(|r| r.len() >= 3)
        .map(|r| to_int_path(&orient_ring(r, true)))
        .collect();
    run_union(&paths, true)
}

/// `subject` minus `clip`.
pub fn difference_polygon_sets(
    subject: &[PolygonWithHoles],
    clip: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let subject_paths = polygon_set_paths(subject);
    if subject_paths.is_empty() {
        return Ok(Vec::new());
    }
    let clip_paths = polygon_set_paths(clip);
    if clip_paths.is_empty() {
        return run_union(&subject_paths, true);
    }
    let mut c = Clipper::new();
    c.strictly_simple = true;
    c.add_paths(&subject_paths, PolyType::Subject, true)?;
    c.add_paths(&clip_paths, PolyType::Clip, true)?;
    let mut tree = PolyTree::default();
    c.execute_tree(
        ClipType::Difference,
        &mut tree,
        PolyFillType::NonZero,
        PolyFillType::NonZero,
    )?;
    Ok(polygons_from_poly_tree(&tree))
}

/// Offset a polygon set by `delta_mm` (positive grows the solid): a miter
/// `ClipperOffset` (limit 2, arc tolerance 2), then a union that keeps the
/// nesting.
pub fn offset_polygon_set(
    polys: &[PolygonWithHoles],
    delta_mm: f64,
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    let paths = polygon_set_paths(polys);
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut co = ClipperOffset::new(2.0, 2.0);
    co.add_paths(&paths, JoinType::Miter, EndType::ClosedPolygon);
    let mut solution = Vec::new();
    co.execute(&mut solution, delta_mm * CLIPPER_SCALE)?;
    run_union(&solution, true)
}

/// `polygonsFromPolyTree`: the `GetFirst`/`GetNext` walk, visiting each walked
/// node *and* recursing into its children, exactly as DragonFruit does. The
/// walk already reaches every descendant, so a node below the top level is
/// visited once per ancestor plus once itself: an island inside a hole comes
/// out several times (a later union folds the copies together; the raft caps
/// draw each copy). See SC-76.
pub fn polygons_from_poly_tree(tree: &PolyTree) -> Vec<PolygonWithHoles> {
    let mut result: Vec<PolygonWithHoles> = Vec::new();
    fn visit(
        tree: &PolyTree,
        node: usize,
        parent: Option<usize>,
        result: &mut Vec<PolygonWithHoles>,
    ) {
        let contour = from_int_path(&tree.nodes[node].polygon);
        let children = tree.nodes[node].childs.clone();
        if contour.len() < 3 {
            for child in children {
                visit(tree, child, parent, result);
            }
            return;
        }
        if tree.is_hole(node) {
            if let Some(p) = parent {
                result[p].holes.push(contour);
            }
            for child in children {
                visit(tree, child, None, result);
            }
            return;
        }
        result.push(PolygonWithHoles {
            outer: contour,
            holes: Vec::new(),
        });
        let me = result.len() - 1;
        for child in children {
            visit(tree, child, Some(me), result);
        }
    }
    let mut node = tree.get_first();
    while let Some(n) = node {
        visit(tree, n, None, &mut result);
        node = tree.get_next(n);
    }
    result
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

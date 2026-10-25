//! DragonFruit `geometry/polygonRaster2d.ts`: the boolean region of a soup of
//! overlapping polygons on a raster grid (scanline fill, one-cell dilation,
//! boundary tracing, Douglas–Peucker), nested into outers and holes.

use std::collections::HashMap;

use super::polygon_set_2d::{PolygonWithHoles, orient_ring};
use crate::three::Vector2;
use crate::three::math_utils::{js_hypot, js_max, js_min};

const TARGET_CELLS_ACROSS: f64 = 1024.0;
const MIN_CELL_MM: f64 = 0.05;
const MAX_CELL_MM: f64 = 0.25;
const DILATE_CELLS: i64 = 1;

pub fn resolve_raster_cell_mm(width_mm: f64, height_mm: f64) -> f64 {
    let longest = js_max(width_mm, height_mm);
    if !longest.is_finite() || longest <= 0.0 {
        return MIN_CELL_MM;
    }
    js_min(
        MAX_CELL_MM,
        js_max(MIN_CELL_MM, longest / TARGET_CELLS_ACROSS),
    )
}

struct Grid {
    cell_mm: f64,
    origin_x: f64,
    origin_y: f64,
    width: i64,
    height: i64,
    mask: Vec<u8>,
}

fn bounds_of_polygons(polygons: &[Vec<Vector2>]) -> Option<(f64, f64, f64, f64)> {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for polygon in polygons {
        for p in polygon {
            if p.x < min_x {
                min_x = p.x;
            }
            if p.x > max_x {
                max_x = p.x;
            }
            if p.y < min_y {
                min_y = p.y;
            }
            if p.y > max_y {
                max_y = p.y;
            }
        }
    }
    min_x.is_finite().then_some((min_x, min_y, max_x, max_y))
}

fn rasterize_edges(grid: &mut Grid, polygon: &[Vector2]) {
    let n = polygon.len();
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        let steps = js_max(
            1.0,
            (js_max((b.x - a.x).abs(), (b.y - a.y).abs()) / grid.cell_mm).ceil(),
        );
        let mut s = 0.0;
        while s <= steps {
            let t = s / steps;
            let ix = ((a.x + (b.x - a.x) * t - grid.origin_x) / grid.cell_mm).floor();
            let iy = ((a.y + (b.y - a.y) * t - grid.origin_y) / grid.cell_mm).floor();
            s += 1.0;
            if ix < 0.0 || iy < 0.0 || ix >= grid.width as f64 || iy >= grid.height as f64 {
                continue;
            }
            grid.mask[(iy as i64 * grid.width + ix as i64) as usize] = 1;
        }
    }
}

fn rasterize_interior(grid: &mut Grid, polygon: &[Vector2], row_crossings: &mut Vec<f64>) {
    let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in polygon {
        if p.y < min_y {
            min_y = p.y;
        }
        if p.y > max_y {
            max_y = p.y;
        }
    }
    let row_start = js_max(0.0, ((min_y - grid.origin_y) / grid.cell_mm).floor()) as i64;
    let row_end = js_min(
        (grid.height - 1) as f64,
        ((max_y - grid.origin_y) / grid.cell_mm).ceil(),
    ) as i64;
    let corners = polygon.len();
    for iy in row_start..=row_end {
        let y = grid.origin_y + (iy as f64 + 0.5) * grid.cell_mm;
        row_crossings.clear();
        for i in 0..corners {
            let a = polygon[i];
            let b = polygon[(i + 1) % corners];
            if (a.y > y) == (b.y > y) {
                continue;
            }
            row_crossings.push(a.x + ((y - a.y) / (b.y - a.y)) * (b.x - a.x));
        }
        let count = row_crossings.len();
        if count < 2 {
            continue;
        }
        if count == 2 {
            if row_crossings[1] < row_crossings[0] {
                row_crossings.swap(0, 1);
            }
        } else {
            // `(left, right) => left - right`: a consistent order, so any stable
            // sort agrees with V8's.
            row_crossings.sort_by(|l, r| l.partial_cmp(r).unwrap_or(std::cmp::Ordering::Equal));
        }
        let row_offset = iy * grid.width;
        let mut c = 0;
        while c + 1 < count {
            let from = js_max(
                0.0,
                ((row_crossings[c] - grid.origin_x) / grid.cell_mm - 0.5).ceil(),
            ) as i64;
            let to = js_min(
                (grid.width - 1) as f64,
                ((row_crossings[c + 1] - grid.origin_x) / grid.cell_mm - 0.5).floor(),
            ) as i64;
            for ix in from..=to {
                grid.mask[(row_offset + ix) as usize] = 1;
            }
            c += 2;
        }
    }
}

fn dilate_rows(grid: &mut Grid, radius: i64) {
    let (w, h) = (grid.width, grid.height);
    for iy in 0..h {
        let row = iy * w;
        let mut run_start = -1;
        for ix in 0..=w {
            let filled = ix < w && grid.mask[(row + ix) as usize] == 1;
            if filled && run_start < 0 {
                run_start = ix;
            }
            if !filled && run_start >= 0 {
                for x in (run_start - radius).max(0)..=(ix - 1 + radius).min(w - 1) {
                    grid.mask[(row + x) as usize] = 1;
                }
                run_start = -1;
            }
        }
    }
}

fn dilate_columns(grid: &mut Grid, radius: i64) {
    let (w, h) = (grid.width, grid.height);
    for ix in 0..w {
        let mut runs: Vec<(i64, i64)> = Vec::new();
        let mut run_start = -1;
        for iy in 0..=h {
            let filled = iy < h && grid.mask[(iy * w + ix) as usize] == 1;
            if filled && run_start < 0 {
                run_start = iy;
            }
            if !filled && run_start >= 0 {
                runs.push((run_start, iy - 1));
                run_start = -1;
            }
        }
        for (start, end) in runs {
            for y in (start - radius).max(0)..=(end + radius).min(h - 1) {
                grid.mask[(y * w + ix) as usize] = 1;
            }
        }
    }
}

/// A `Map` keyed by corner: insertion-ordered, a re-`set` keeps the slot.
#[derive(Default)]
struct EdgeMap {
    order: Vec<(i64, i64)>,
    next: HashMap<(i64, i64), (i64, i64)>,
}

impl EdgeMap {
    fn set(&mut self, from: (i64, i64), to: (i64, i64)) {
        if self.next.insert(from, to).is_none() {
            self.order.push(from);
        }
    }
}

fn trace_mask_loops(grid: &Grid) -> Vec<Vec<Vector2>> {
    let (w, h) = (grid.width, grid.height);
    let at = |x: i64, y: i64| grid.mask[(y * w + x) as usize] == 1;
    let mut edges = EdgeMap::default();
    for iy in 0..h {
        for ix in 0..w {
            if !at(ix, iy) {
                continue;
            }
            let left = ix > 0 && at(ix - 1, iy);
            let right = ix + 1 < w && at(ix + 1, iy);
            let below = iy > 0 && at(ix, iy - 1);
            let above = iy + 1 < h && at(ix, iy + 1);
            if !left {
                edges.set((ix, iy + 1), (ix, iy));
            }
            if !right {
                edges.set((ix + 1, iy), (ix + 1, iy + 1));
            }
            if !below {
                edges.set((ix, iy), (ix + 1, iy));
            }
            if !above {
                edges.set((ix + 1, iy + 1), (ix, iy + 1));
            }
        }
    }
    let mut loops = Vec::new();
    // Iterating a Map while deleting from it visits the survivors in order.
    for k in 0..edges.order.len() {
        let start = edges.order[k];
        if !edges.next.contains_key(&start) {
            continue;
        }
        let mut lp = Vec::new();
        let mut current = Some(start);
        let mut guard = edges.next.len() as i64 + 1;
        while let Some(cur) = current {
            let g = guard;
            guard -= 1;
            if g <= 0 {
                break;
            }
            let Some(next) = edges.next.remove(&cur) else {
                break;
            };
            lp.push(Vector2::new(
                grid.origin_x + cur.0 as f64 * grid.cell_mm,
                grid.origin_y + cur.1 as f64 * grid.cell_mm,
            ));
            current = Some(next);
            if next == start {
                break;
            }
        }
        if lp.len() >= 4 {
            loops.push(lp);
        }
    }
    loops
}

/// Ramer–Douglas–Peucker over an open chain, endpoints kept.
fn simplify_chain(points: &[Vector2], tolerance: f64) -> Vec<Vector2> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let first = points[0];
    let last = points[points.len() - 1];
    let dx = last.x - first.x;
    let dy = last.y - first.y;
    let length = js_hypot(&[dx, dy]);
    let mut worst: Option<usize> = None;
    let mut worst_distance = tolerance;
    for (i, point) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let distance = if length <= 1e-9 {
            point.distance_to(first)
        } else {
            (dy * point.x - dx * point.y + last.x * first.y - last.y * first.x).abs() / length
        };
        if distance > worst_distance {
            worst_distance = distance;
            worst = Some(i);
        }
    }
    let Some(worst) = worst else {
        return vec![first, last];
    };
    let mut left = simplify_chain(&points[..=worst], tolerance);
    left.pop();
    left.extend(simplify_chain(&points[worst..], tolerance));
    left
}

fn simplify_loop(lp: &[Vector2], tolerance: f64) -> Vec<Vector2> {
    if lp.len() <= 4 {
        return lp.to_vec();
    }
    let mut farthest = 0;
    let mut farthest_distance = -1.0;
    for (i, p) in lp.iter().enumerate().skip(1) {
        let distance = lp[0].distance_to_squared(*p);
        if distance > farthest_distance {
            farthest_distance = distance;
            farthest = i;
        }
    }
    let first_chain = &lp[..=farthest];
    let mut second_chain = lp[farthest..].to_vec();
    second_chain.push(lp[0]);
    let mut simplified = simplify_chain(first_chain, tolerance);
    simplified.pop();
    let mut second = simplify_chain(&second_chain, tolerance);
    second.pop();
    simplified.extend(second);
    if simplified.len() >= 3 {
        simplified
    } else {
        lp.to_vec()
    }
}

fn ring_contains_point(ring: &[Vector2], x: f64, y: f64) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n == 0 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > y) != (b.y > y) && x < ((b.x - a.x) * (y - a.y)) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn nest_loops(loops: Vec<Vec<Vector2>>) -> Vec<PolygonWithHoles> {
    let mut with_area: Vec<(Vec<Vector2>, f64)> = loops
        .into_iter()
        .map(|ring| {
            let mut total = 0.0;
            for i in 0..ring.len() {
                let (p, next) = (ring[i], ring[(i + 1) % ring.len()]);
                // `total + point.x * next.y - next.x * point.y`: left to
                // right, not `total + (a - b)`.
                total = total + p.x * next.y - next.x * p.y;
            }
            let area = (total / 2.0).abs();
            (ring, area)
        })
        .collect();
    // `right.area - left.area`, stable.
    with_area.sort_by(|l, r| r.1.partial_cmp(&l.1).unwrap_or(std::cmp::Ordering::Equal));

    let depth: Vec<usize> = (0..with_area.len())
        .map(|i| {
            let p = with_area[i].0[0];
            (0..i)
                .filter(|&j| ring_contains_point(&with_area[j].0, p.x, p.y))
                .count()
        })
        .collect();

    let mut polygons: Vec<PolygonWithHoles> = Vec::new();
    let mut created: HashMap<usize, usize> = HashMap::new();
    for i in 0..with_area.len() {
        if !depth[i].is_multiple_of(2) {
            continue;
        }
        polygons.push(PolygonWithHoles {
            outer: orient_ring(&with_area[i].0, true),
            holes: Vec::new(),
        });
        created.insert(i, polygons.len() - 1);
    }
    for i in 0..with_area.len() {
        if depth[i].is_multiple_of(2) {
            continue;
        }
        let p = with_area[i].0[0];
        let parent = (0..i)
            .find(|&j| depth[j] + 1 == depth[i] && ring_contains_point(&with_area[j].0, p.x, p.y));
        if let Some(&k) = parent.and_then(|j| created.get(&j)) {
            polygons[k].holes.push(orient_ring(&with_area[i].0, false));
        }
    }
    polygons.retain(|poly| poly.outer.len() >= 3);
    polygons
}

/// `rasterizePolygonsToRegion`: a polygon soup as simple, nested rings.
pub fn rasterize_polygons_to_region(polygons: &[Vec<Vector2>]) -> Vec<PolygonWithHoles> {
    let Some((min_x, min_y, max_x, max_y)) = bounds_of_polygons(polygons) else {
        return Vec::new();
    };
    let width_mm = max_x - min_x;
    let height_mm = max_y - min_y;
    let cell_mm = resolve_raster_cell_mm(width_mm, height_mm);
    let margin = cell_mm * 4.0;
    let width = js_max(1.0, ((width_mm + margin * 2.0) / cell_mm).ceil()) as i64;
    let height = js_max(1.0, ((height_mm + margin * 2.0) / cell_mm).ceil()) as i64;
    let mut grid = Grid {
        cell_mm,
        origin_x: min_x - margin,
        origin_y: min_y - margin,
        width,
        height,
        mask: vec![0; (width * height) as usize],
    };
    let mut row_crossings = Vec::new();
    for polygon in polygons {
        if polygon.len() < 3 {
            continue;
        }
        rasterize_edges(&mut grid, polygon);
        rasterize_interior(&mut grid, polygon, &mut row_crossings);
    }
    dilate_rows(&mut grid, DILATE_CELLS);
    dilate_columns(&mut grid, DILATE_CELLS);
    let loops: Vec<Vec<Vector2>> = trace_mask_loops(&grid)
        .iter()
        .map(|lp| simplify_loop(lp, cell_mm))
        .collect();
    nest_loops(loops)
}

//! How hard a peel drags a posed part sideways, and how much its contact
//! with the plate holds against that.
//!
//! A part fails by turning about an edge of what it stands on. Every face
//! looking down pulls sideways as it peels, by its area times `sin θ`, at
//! its height above the part's base: the drag moment. About an edge with
//! outward direction `u` a face adds `area · (n_xy · u) · height` when it
//! pushes that way. The edge that turns the part most easily is the one
//! where gravity's lever (volume times how far the centroid is inside the
//! edge, over the edge's moment) is least. About that edge the plate holds
//! with its contact area times how far the contact's centre is inside the
//! edge; over the edge's moment that is the adhesion ratio: the part lifts
//! when the peel pressure over the plate's grip is above it.

use serde::{Deserialize, Serialize};

use crate::Surface;

/// The band above the part's lowest point it stands on (mm), without a raft.
const BEARING_BAND_MM: f32 = 2.0;
/// With a raft the part stands on its shadow, taken on this grid (mm).
const SHADOW_CELL_MM: f32 = 0.5;

/// What [`stability`] finds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StabilityReport {
    /// The drag moment of every face looking down, summed (mm³).
    pub drag_moment_mm3: f64,
    /// Contact area times its centre's depth inside the weakest edge, over
    /// that edge's moment; infinite when nothing drags about it.
    pub adhesion_ratio: f64,
    /// What the part stands on: its area (mm²) and edges (0: a point or a
    /// line, nothing to stand on).
    pub bearing_area_mm2: f32,
    pub bearing_edges: usize,
    /// The volume the surface holds (mm³).
    pub volume_mm3: f64,
}

/// The report for `surface` as posed, for a self-support angle (deg); `None`
/// without a triangle that has area. With a raft the part stands on its whole
/// shadow (an underestimate of the raft), otherwise on the hull of what lies
/// within 2 mm of its lowest point. Faces without area are left out of both.
pub fn stability(surface: &Surface<'_>, has_raft: bool) -> Option<StabilityReport> {
    let n = surface.triangles.len();
    let with_area: Vec<usize> = (0..n)
        .filter(|&f| surface.normal_and_area(f).1 > 0.0)
        .collect();
    let z_min = with_area
        .iter()
        .flat_map(|&f| surface.corners(f))
        .map(|p| p[2])
        .fold(f32::INFINITY, f32::min);
    if !z_min.is_finite() {
        return None;
    }

    let mut stand: Vec<[f32; 2]> = Vec::new();
    if has_raft {
        let mut cells = std::collections::HashSet::new();
        for p in with_area.iter().flat_map(|&f| surface.corners(f)) {
            let cell = (
                (p[0] / SHADOW_CELL_MM).round() as i32,
                (p[1] / SHADOW_CELL_MM).round() as i32,
            );
            if cells.insert(cell) {
                stand.push([p[0], p[1]]);
            }
        }
    } else {
        stand.extend(
            with_area
                .iter()
                .flat_map(|&f| surface.corners(f))
                .filter(|p| p[2] - z_min <= BEARING_BAND_MM)
                .map(|p| [p[0], p[1]]),
        );
    }
    let hull = convex_hull(stand);
    let (bearing_area, contact_centre) = area_and_centre(&hull);
    // Each edge: its outward direction (the hull runs counter-clockwise) and
    // a point on it.
    let edges: Vec<([f64; 2], [f64; 2])> = (0..hull.len())
        .filter_map(|i| {
            let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
            let d = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])];
            let length = d[0].hypot(d[1]);
            (length > 1e-9).then(|| {
                (
                    [d[1] / length, -d[0] / length],
                    [f64::from(a[0]), f64::from(a[1])],
                )
            })
        })
        .collect();

    let mut moment_about = vec![0.0f64; edges.len()];
    let mut six_volume = 0.0f64;
    let mut moments = [0.0f64; 3];
    let mut drag = 0.0f64;
    for f in 0..n {
        let [a, b, c] = surface.corners(f).map(|p| p.map(f64::from));
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        six_volume += det;
        for k in 0..3 {
            moments[k] += det * (a[k] + b[k] + c[k]) * 0.25;
        }
        let (normal, area) = surface.normal_and_area(f);
        let sin = f64::from(normal[0]).hypot(f64::from(normal[1]));
        if normal[2] >= 0.0 || sin <= 1e-6 {
            continue;
        }
        let height = (a[2] + b[2] + c[2]) / 3.0 - f64::from(z_min);
        if height <= 0.0 {
            continue;
        }
        let area = f64::from(area);
        drag += area * sin * height;
        let push = [f64::from(normal[0]), f64::from(normal[1])];
        for (sum, (out, _)) in moment_about.iter_mut().zip(&edges) {
            let along = push[0] * out[0] + push[1] * out[1];
            if along > 0.0 {
                *sum += area * along * height;
            }
        }
    }
    let volume = (six_volume / 6.0).abs();
    let centroid = if six_volume.abs() > 1e-9 {
        [moments[0] / six_volume, moments[1] / six_volume]
    } else {
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in surface.positions {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        [
            f64::from((lo[0] + hi[0]) * 0.5),
            f64::from((lo[1] + hi[1]) * 0.5),
        ]
    };

    // The weakest edge: gravity's least lever about it.
    let mut weakest: Option<(f64, f64, f64)> = None;
    for ((out, at), &sum) in edges.iter().zip(&moment_about) {
        let inside = |p: [f64; 2]| (at[0] - p[0]) * out[0] + (at[1] - p[1]) * out[1];
        let lever = if volume <= 1e-9 {
            0.0
        } else if sum > 1e-9 {
            volume * inside(centroid) / sum
        } else {
            f64::INFINITY
        };
        let contact_depth = contact_centre.map_or(0.0, inside);
        if weakest.is_none_or(|w| lever < w.0) {
            weakest = Some((lever, contact_depth, sum));
        }
    }
    let (_, contact_depth, edge_moment) = weakest.unwrap_or((0.0, 0.0, 0.0));
    Some(StabilityReport {
        drag_moment_mm3: drag,
        adhesion_ratio: if edge_moment > 1e-9 {
            // The contact depth was f32 in the first version of this report.
            f64::from(bearing_area) * f64::from(contact_depth as f32) / edge_moment
        } else {
            f64::INFINITY
        },
        bearing_area_mm2: bearing_area,
        bearing_edges: edges.len(),
        volume_mm3: volume,
    })
}

/// The convex hull, counter-clockwise, without points on its edges; empty
/// for fewer than three points not in a line.
fn convex_hull(mut points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    points.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return Vec::new();
    }
    let turn = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Vec<[f32; 2]> = Vec::with_capacity(points.len());
    for pass in [&points[..], &points[..points.len() - 1]] {
        // The upper chain may pop back to the lower chain's last point.
        let floor = hull.len().max(1);
        let ordered: Box<dyn Iterator<Item = &[f32; 2]>> = if hull.is_empty() {
            Box::new(pass.iter())
        } else {
            Box::new(pass.iter().rev())
        };
        for &p in ordered {
            while hull.len() > floor && turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
    }
    hull.pop();
    if hull.len() < 3 { Vec::new() } else { hull }
}

/// A polygon's area and the centre of that area (none without area).
fn area_and_centre(polygon: &[[f32; 2]]) -> (f32, Option<[f64; 2]>) {
    if polygon.len() < 3 {
        return (0.0, None);
    }
    let (mut twice, mut cx, mut cy) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..polygon.len() {
        let (p, q) = (
            polygon[i].map(f64::from),
            polygon[(i + 1) % polygon.len()].map(f64::from),
        );
        let cross = p[0] * q[1] - q[0] * p[1];
        twice += cross;
        cx += (p[0] + q[0]) * cross;
        cy += (p[1] + q[1]) * cross;
    }
    let area = (twice.abs() / 2.0) as f32;
    let centre = (twice.abs() > 1e-9).then(|| [cx / (3.0 * twice), cy / (3.0 * twice)]);
    (area, centre)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hull_drops_the_points_inside_and_on_its_edges() {
        let hull = convex_hull(vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [1.0, 0.0],
            [2.0, 2.0],
            [0.0, 2.0],
            [1.0, 1.0],
        ]);
        assert_eq!(hull, vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]]);
        let (area, centre) = area_and_centre(&hull);
        assert_eq!(area, 4.0);
        assert_eq!(centre, Some([1.0, 1.0]));
    }
}

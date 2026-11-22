//! Where models stand in a raft's way: the plan of every triangle part of
//! them has between the plate and the raft's top, unioned exactly and grown
//! by a small margin, across the models.

use super::polygon_set_2d::{
    PolygonError, PolygonWithHoles, offset_polygon_set, orient_ring, ring_to_polygon,
    union_polygon_sets,
};
use bread_support_geom::geom::buffer_geometry::BufferGeometry;
use bread_support_geom::geom::{Matrix4, Quaternion, Vec3, Vector2};

/// How far a raft stays from a model standing on the plate (mm).
pub const MODEL_PLATE_CLEARANCE_MM: f64 = 1.0;

/// A model's place on the plate: position, turn about the world's axes
/// (X, then Y, then Z), and scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelTransform {
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
}

/// A model's mesh, the point it turns about, and its place.
#[derive(Debug, Clone)]
pub struct PlateFootprintSource<'a> {
    pub geometry: &'a BufferGeometry,
    pub center: Vec3,
    pub transform: ModelTransform,
    pub visible: Option<bool>,
}

/// The turn by `rotation` (radians) about the world's X, then Y, then Z
/// (non-finite angles count as none).
pub fn quaternion_from_global_euler(rotation: Vec3) -> Quaternion {
    let angle = |a: f64| if a.is_finite() { a } else { 0.0 };
    let about = |axis: Vec3, a: f64| Quaternion::set_from_axis_angle(axis, angle(a));
    about(Vec3::new(0.0, 0.0, 1.0), rotation.z)
        .multiply(about(Vec3::new(0.0, 1.0, 0.0), rotation.y))
        .multiply(about(Vec3::new(1.0, 0.0, 0.0), rotation.x))
}

/// The plan of what of triangle `abc` lies between heights `low` and
/// `high`, when it has any area.
fn plan_between(corners: [[f64; 3]; 3], low: f64, high: f64) -> Option<Vec<Vector2>> {
    let mut polygon: Vec<[f64; 3]> = corners.to_vec();
    for (limit, above) in [(low, true), (high, false)] {
        let keeps = |p: &[f64; 3]| if above { p[2] >= limit } else { p[2] <= limit };
        let mut clipped = Vec::with_capacity(polygon.len() + 2);
        for i in 0..polygon.len() {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            if keeps(&a) {
                clipped.push(a);
            }
            if keeps(&a) != keeps(&b) {
                let t = (limit - a[2]) / (b[2] - a[2]);
                clipped.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, limit]);
            }
        }
        polygon = clipped;
        if polygon.len() < 3 {
            return None;
        }
    }
    let plan: Vec<Vector2> = polygon.iter().map(|p| Vector2::new(p[0], p[1])).collect();
    let n = plan.len();
    let twice: f64 = (0..n)
        .map(|i| plan[i].x * plan[(i + 1) % n].y - plan[(i + 1) % n].x * plan[i].y)
        .sum();
    (twice.abs() >= 1e-9).then_some(plan)
}

/// How much a model's shadow is grown: about two thousandths of its size,
/// from 0.11 to 0.55 mm, so shadows that only touch at a corner join (and the
/// raft stays no closer than with the grid this replaces).
fn shadow_margin(rings: &[Vec<Vector2>]) -> f64 {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in rings.iter().flatten() {
        lo = [lo[0].min(p.x), lo[1].min(p.y)];
        hi = [hi[0].max(p.x), hi[1].max(p.y)];
    }
    let longest = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if !longest.is_finite() || longest <= 0.0 {
        return 0.11;
    }
    2.2 * (longest / 1024.0).clamp(0.05, 0.25)
}

/// The plan the visible models take between the plate and `band_top_mm`.
pub fn collect_model_plate_footprint(
    sources: &[PlateFootprintSource],
    band_top_mm: f64,
) -> Result<Vec<PolygonWithHoles>, PolygonError> {
    if !band_top_mm.is_finite() || band_top_mm <= 0.0 {
        return Ok(Vec::new());
    }
    let mut all: Vec<PolygonWithHoles> = Vec::new();
    for source in sources.iter().filter(|s| s.visible != Some(false)) {
        let t = &source.transform;
        let m = Matrix4::compose(
            t.position,
            quaternion_from_global_euler(t.rotation),
            t.scale,
        );
        let e = m.elements;
        let Some(position) = source.geometry.position.as_deref() else {
            continue;
        };
        let c = source.center;
        let world = |v: usize| {
            let (x, y, z) = (
                f64::from(position[v * 3]) - c.x,
                f64::from(position[v * 3 + 1]) - c.y,
                f64::from(position[v * 3 + 2]) - c.z,
            );
            [
                e[0] * x + e[4] * y + e[8] * z + e[12],
                e[1] * x + e[5] * y + e[9] * z + e[13],
                e[2] * x + e[6] * y + e[10] * z + e[14],
            ]
        };
        let count = match &source.geometry.index {
            Some(index) => index.len() / 3,
            None => position.len() / 9,
        };
        let corner = |t: usize, k: usize| match &source.geometry.index {
            Some(index) => index[t * 3 + k] as usize,
            None => t * 3 + k,
        };
        let rings: Vec<Vec<Vector2>> = (0..count)
            .filter_map(|t| {
                let corners = [0, 1, 2].map(|k| world(corner(t, k)));
                let lowest = corners.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
                let highest = corners
                    .iter()
                    .map(|p| p[2])
                    .fold(f64::NEG_INFINITY, f64::max);
                if highest < 0.0 || lowest > band_top_mm {
                    return None;
                }
                plan_between(corners, 0.0, band_top_mm)
            })
            .collect();
        if rings.is_empty() {
            continue;
        }
        let margin = shadow_margin(&rings);
        let shadow: Vec<PolygonWithHoles> = rings
            .iter()
            .map(|r| ring_to_polygon(orient_ring(r, true)))
            .collect();
        all.extend(offset_polygon_set(&union_polygon_sets(&shadow)?, margin)?);
    }
    if all.len() <= 1 {
        return Ok(all);
    }
    union_polygon_sets(&all)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box standing on the plate casts its own plan, grown a little; one
    /// lifted over the band casts none.
    #[test]
    fn a_box_on_the_plate_casts_its_plan() {
        let mut positions: Vec<f64> = Vec::new();
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let corner = |i: usize| {
            [
                if i & 1 == 0 { 0.0 } else { 10.0 },
                if i & 2 == 0 { 0.0 } else { 6.0 },
                if i & 4 == 0 { 0.0 } else { 4.0 },
            ]
        };
        for q in quads {
            for k in [0, 1, 2, 0, 2, 3] {
                positions.extend(corner(q[k]));
            }
        }
        let mut geometry = BufferGeometry::default();
        geometry.set_position(&positions);
        let source = |z: f64| PlateFootprintSource {
            geometry: &geometry,
            center: Vec3::new(0.0, 0.0, 0.0),
            transform: ModelTransform {
                position: Vec3::new(0.0, 0.0, z),
                rotation: Vec3::new(0.0, 0.0, 0.0),
                scale: Vec3::new(1.0, 1.0, 1.0),
            },
            visible: None,
        };
        let plan = collect_model_plate_footprint(&[source(0.0)], 0.85).unwrap();
        assert_eq!(plan.len(), 1);
        let area = super::super::polygon_set_2d::polygon_set_area_mm2(&plan);
        assert!(area > 60.0 && area < 64.0, "{area}");
        assert!(
            collect_model_plate_footprint(&[source(5.0)], 0.85)
                .unwrap()
                .is_empty()
        );
    }
}

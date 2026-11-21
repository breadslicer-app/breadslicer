//! DragonFruit `geometry/modelPlateFootprint.ts`: where a model meets the
//! raft, as the XY footprint of its triangles inside the raft's height band
//! (plate to raft top), unioned across models.
//!
//! DragonFruit memoises three caches (bottom-triangle index per geometry,
//! local and world shadows by quantised key). Every one returns what a fresh
//! computation returns for the inputs it is keyed on, so the port computes
//! fresh; the one difference (a local-shadow key quantised to 1e-5 mm can
//! serve a band a few nanometres away) is noted in BS-64.

use super::polygon_raster_2d::rasterize_polygons_to_region;
use super::polygon_set_2d::{PolygonWithHoles, union_polygon_sets};
use crate::clipper::ClipperError;
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::{Matrix4, Quaternion, Vec3, Vector2};

pub const MODEL_PLATE_CLEARANCE_MM: f64 = 1.0;

/// Extra local Z scanned when building a geometry's bottom-triangle index.
const BOTTOM_WINDOW_MM: f64 = 12.0;

/// `ModelTransform`: position, the global-axis Euler components, scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelTransform {
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
}

/// `PlateFootprintSource`.
#[derive(Debug, Clone)]
pub struct PlateFootprintSource<'a> {
    pub geometry: &'a BufferGeometry,
    pub center: Vec3,
    pub transform: ModelTransform,
    pub visible: Option<bool>,
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// DragonFruit `utils/rotation.ts` `quaternionFromGlobalEuler`: extrinsic XYZ,
/// `qz * qy * qx`.
pub fn quaternion_from_global_euler(rotation: Vec3) -> Quaternion {
    let (x, y, z) = (
        finite_or_zero(rotation.x),
        finite_or_zero(rotation.y),
        finite_or_zero(rotation.z),
    );
    let qx = Quaternion::set_from_axis_angle(Vec3::new(1.0, 0.0, 0.0), x);
    let qy = Quaternion::set_from_axis_angle(Vec3::new(0.0, 1.0, 0.0), y);
    let qz = Quaternion::set_from_axis_angle(Vec3::new(0.0, 0.0, 1.0), z);
    qz.multiply(qy).multiply(qx)
}

fn is_plate_facing_rotation(t: &ModelTransform) -> bool {
    t.rotation.x.abs() < 1e-6 && t.rotation.y.abs() < 1e-6 && t.scale.z.abs() > 1e-9
}

fn to_world_point(p: Vector2, m: &Matrix4) -> Vector2 {
    let e = &m.elements;
    Vector2::new(
        e[0] * p.x + e[4] * p.y + e[12],
        e[1] * p.x + e[5] * p.y + e[13],
    )
}

/// `geometry.boundingBox` (`computeBoundingBox`): `Math.min`/`Math.max` over
/// the stored floats.
fn bounding_box(position: &[f32]) -> (Vec3, Vec3) {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in position.as_chunks::<3>().0 {
        let p = Vec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

fn world_z_range(geometry: &BufferGeometry, center: Vec3, m: &Matrix4) -> Option<(f64, f64)> {
    let (bmin, bmax) = bounding_box(geometry.position.as_deref().unwrap_or(&[]));
    let e = &m.elements;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for corner in 0..8 {
        let x = (if corner & 1 != 0 { bmax.x } else { bmin.x }) - center.x;
        let y = (if corner & 2 != 0 { bmax.y } else { bmin.y }) - center.y;
        let z = (if corner & 4 != 0 { bmax.z } else { bmin.z }) - center.z;
        let world_z = e[2] * x + e[6] * y + e[10] * z + e[14];
        if world_z < min {
            min = world_z;
        }
        if world_z > max {
            max = world_z;
        }
    }
    min.is_finite().then_some((min, max))
}

fn vertex_index(geometry: &BufferGeometry, triangle: usize, corner: usize) -> usize {
    match &geometry.index {
        Some(index) => index[triangle * 3 + corner] as usize,
        None => triangle * 3 + corner,
    }
}

fn triangle_count(geometry: &BufferGeometry) -> usize {
    match &geometry.index {
        Some(index) => index.len() / 3,
        None => geometry.vertex_count() / 3,
    }
}

/// `getBottomIndex`: triangles with a corner within the bottom window.
fn get_bottom_index(geometry: &BufferGeometry, required_top_mm: f64) -> Option<Vec<usize>> {
    let position = geometry.position.as_deref()?;
    let count = position.len() / 3;
    if count < 3 {
        return None;
    }
    let z_at = |v: usize| position[v * 3 + 2] as f64;
    let mut min_z = f64::INFINITY;
    for v in 0..count {
        let z = z_at(v);
        if z < min_z {
            min_z = z;
        }
    }
    if !min_z.is_finite() {
        return None;
    }
    let top_mm = crate::three::math_utils::js_max(BOTTOM_WINDOW_MM, required_top_mm - min_z);
    let limit = min_z + top_mm;
    let mut collected = Vec::new();
    for triangle in 0..triangle_count(geometry) {
        let (a, b, c) = (
            vertex_index(geometry, triangle, 0),
            vertex_index(geometry, triangle, 1),
            vertex_index(geometry, triangle, 2),
        );
        if z_at(a) <= limit || z_at(b) <= limit || z_at(c) <= limit {
            collected.push(triangle);
        }
    }
    Some(collected)
}

/// Sutherland–Hodgman clip of a flat XYZ polygon to `zLo <= z <= zHi`, as flat XY.
fn clip_slab_to_flat_xy(verts: &[f64; 9], z_lo: f64, z_hi: f64) -> Vec<f64> {
    let mut input: Vec<f64> = verts.to_vec();
    for (limit, keep_above) in [(z_lo, true), (z_hi, false)] {
        let mut output = Vec::new();
        let mut i = 0;
        while i < input.len() {
            let j = (i + 3) % input.len();
            let (za, zb) = (input[i + 2], input[j + 2]);
            let a_inside = if keep_above { za >= limit } else { za <= limit };
            let b_inside = if keep_above { zb >= limit } else { zb <= limit };
            if a_inside {
                output.extend([input[i], input[i + 1], input[i + 2]]);
            }
            if a_inside != b_inside {
                let t = (limit - za) / (zb - za);
                output.extend([
                    input[i] + (input[j] - input[i]) * t,
                    input[i + 1] + (input[j + 1] - input[i + 1]) * t,
                    limit,
                ]);
            }
            i += 3;
        }
        input = output;
        if input.len() < 9 {
            return Vec::new();
        }
    }
    input.chunks(3).flat_map(|p| [p[0], p[1]]).collect()
}

/// `accumulateShadow`: every candidate triangle inside the band, clipped to
/// it, as XY rings.
fn accumulate_shadow(
    geometry: &BufferGeometry,
    center: Vec3,
    to_band_frame: impl Fn(f64, f64, f64) -> [f64; 3],
    z_lo: f64,
    z_hi: f64,
    triangles: Option<&[usize]>,
) -> Vec<Vec<Vector2>> {
    let Some(position) = geometry.position.as_deref() else {
        return Vec::new();
    };
    if position.len() / 3 < 3 {
        return Vec::new();
    }
    let count = triangle_count(geometry);
    let candidates = triangles.map_or(count, |t| t.len());
    let mut polygons = Vec::new();
    let mut frame = [0.0f64; 9];
    for i in 0..candidates {
        let triangle = triangles.map_or(i, |t| t[i]);
        if triangle >= count {
            continue;
        }
        let mut min_z = f64::INFINITY;
        let mut max_z = f64::NEG_INFINITY;
        for corner in 0..3 {
            let v = vertex_index(geometry, triangle, corner);
            let out = to_band_frame(
                position[v * 3] as f64 - center.x,
                position[v * 3 + 1] as f64 - center.y,
                position[v * 3 + 2] as f64 - center.z,
            );
            frame[corner * 3..corner * 3 + 3].copy_from_slice(&out);
            let z = out[2];
            if min_z > z {
                min_z = z;
            }
            if max_z < z {
                max_z = z;
            }
        }
        if max_z < z_lo || min_z > z_hi {
            continue;
        }
        let xy = if min_z >= z_lo && max_z <= z_hi {
            vec![frame[0], frame[1], frame[3], frame[4], frame[6], frame[7]]
        } else {
            clip_slab_to_flat_xy(&frame, z_lo, z_hi)
        };
        if xy.len() < 6 {
            continue;
        }
        let mut area = 0.0;
        let mut p = 0;
        while p < xy.len() {
            let q = (p + 2) % xy.len();
            area += xy[p] * xy[q + 1] - xy[q] * xy[p + 1];
            p += 2;
        }
        if area.abs() < 1e-9 {
            continue;
        }
        polygons.push(xy.chunks(2).map(|c| Vector2::new(c[0], c[1])).collect());
    }
    polygons
}

fn rings_to_polygons(rings: &[Vec<Vector2>]) -> Vec<PolygonWithHoles> {
    if rings.is_empty() {
        return Vec::new();
    }
    rasterize_polygons_to_region(rings)
}

fn local_shadow_polygons(
    geometry: &BufferGeometry,
    center: Vec3,
    z_lo: f64,
    z_hi: f64,
) -> Vec<PolygonWithHoles> {
    let Some(bottom) = get_bottom_index(geometry, z_hi) else {
        return Vec::new();
    };
    let rings = accumulate_shadow(
        geometry,
        center,
        |x, y, z| [x, y, z],
        z_lo,
        z_hi,
        Some(&bottom),
    );
    rings_to_polygons(&rings)
}

/// `collectModelPlateFootprint(sources, bandTopMm)`.
pub fn collect_model_plate_footprint(
    sources: &[PlateFootprintSource],
    band_top_mm: f64,
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    if !band_top_mm.is_finite() || band_top_mm <= 0.0 {
        return Ok(Vec::new());
    }
    let mut sets: Vec<PolygonWithHoles> = Vec::new();
    for source in sources {
        if source.visible == Some(false) {
            continue;
        }
        let (geometry, center, t) = (source.geometry, source.center, &source.transform);
        let m = Matrix4::compose(
            t.position,
            quaternion_from_global_euler(t.rotation),
            t.scale,
        );
        let Some((zmin, zmax)) = world_z_range(geometry, center, &m) else {
            continue;
        };
        if zmin > band_top_mm || zmax < 0.0 {
            continue;
        }
        let polys = if is_plate_facing_rotation(t) {
            let scale_z = t.scale.z;
            let za = (0.0 - t.position.z) / scale_z;
            let zb = (band_top_mm - t.position.z) / scale_z;
            let lo = crate::three::math_utils::js_min(za, zb);
            let hi = crate::three::math_utils::js_max(za, zb);
            local_shadow_polygons(geometry, center, lo, hi)
                .into_iter()
                .map(|poly| PolygonWithHoles {
                    outer: poly.outer.iter().map(|&p| to_world_point(p, &m)).collect(),
                    holes: poly
                        .holes
                        .iter()
                        .map(|h| h.iter().map(|&p| to_world_point(p, &m)).collect())
                        .collect(),
                })
                .collect()
        } else {
            let e = m.elements;
            let to_world = move |x: f64, y: f64, z: f64| {
                [
                    e[0] * x + e[4] * y + e[8] * z + e[12],
                    e[1] * x + e[5] * y + e[9] * z + e[13],
                    e[2] * x + e[6] * y + e[10] * z + e[14],
                ]
            };
            rings_to_polygons(&accumulate_shadow(
                geometry,
                center,
                to_world,
                0.0,
                band_top_mm,
                None,
            ))
        };
        sets.extend(polys);
    }
    if sets.len() <= 1 {
        return Ok(sets);
    }
    union_polygon_sets(&sets)
}

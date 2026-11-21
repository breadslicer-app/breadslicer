//! Overhang regions and steep faces.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::outline::inset_outline;
use crate::{Surface, is_overhang_normal_z, overhang_normal_z_limit};

/// The steepest a steep face may be (deg from horizontal): past it the face
/// is a wall, which a support rising straight up would only graze.
const STEEP_FACE_MAX_ANGLE_DEG: f32 = 85.0;
/// The least area (mm²) a steep face needs before a peel can lever the
/// part over on it.
const STEEP_FACE_MIN_AREA_MM2: f32 = 150.0;
/// How far a triangle's normal may turn from the face's mean so far (deg)
/// and still belong to it: a crease ends a face.
const STEEP_FACE_NORMAL_TOLERANCE_DEG: f32 = 20.0;
/// How far the outline is drawn in (mm), so a support's contact disc sits on
/// the surface.
const OUTLINE_INSET_MM: f32 = 0.25;

/// The shadow of a region on the plate, as pixels, with the height of the
/// region's surface over each.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FootprintMask {
    pub width: u32,
    pub height: u32,
    /// World XY of the first pixel's centre (mm).
    pub origin_x: f32,
    pub origin_y: f32,
    pub px_mm: f32,
    /// Row by row, 1 where the region's shadow covers the pixel's centre.
    pub data: Vec<u8>,
    /// Row by row, the height of the region's surface over each covered
    /// pixel's centre (of the first of its triangles there).
    pub surface_z: Vec<f32>,
}

/// A group of overhang triangles, what the placement covers as one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverhangRegion {
    /// Its triangles, ascending.
    pub triangle_ids: Vec<u32>,
    /// Their area (mm²).
    pub area_mm2: f32,
    /// Their shadow's area on the plate (mm²), which the peel pulls on.
    pub projected_area_mm2: f32,
    /// Area-weighted mean angle from horizontal (deg).
    pub angle_deg: f32,
    /// Area-weighted mean unit normal (pointing out of the model).
    pub normal: [f32; 3],
    pub xy_min: [f32; 2],
    pub xy_max: [f32; 2],
    /// Lowest and highest corner (mm): the peel starts at `min_z`.
    pub min_z: f32,
    pub max_z: f32,
    pub footprint: FootprintMask,
    /// The sideways peel drag it carries about the part's base (mm³):
    /// `Σ area · sin θ · height` over its triangles.
    pub drag_moment_mm3: f32,
    /// Which way that drag pushes the part (deg in XY, 0 = +X).
    pub drag_dir_deg: f32,
    /// A large face past the self-support angle ([`steep_faces`]), covered
    /// only to hold the part against toppling.
    pub steep_flat: bool,
    /// Its outlines, drawn in by 0.25 mm on the surface, each closed.
    #[serde(default)]
    pub perimeter_loops: Vec<Vec<[f32; 3]>>,
}

/// Each edge's triangles, by sorting the edges once: `keys` ascending, the
/// triangles of edge `e` in `faces[starts[e]..starts[e + 1]]`, ascending.
pub(crate) struct EdgeFaces {
    keys: Vec<u64>,
    starts: Vec<u32>,
    faces: Vec<u32>,
}

pub(crate) fn edge_key(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    (u64::from(lo) << 32) | u64::from(hi)
}

impl EdgeFaces {
    pub(crate) fn new(triangles: &[[u32; 3]]) -> Self {
        let mut pairs: Vec<(u64, u32)> = triangles
            .par_iter()
            .enumerate()
            .flat_map_iter(|(f, &[a, b, c])| {
                [(a, b), (b, c), (c, a)].map(|(u, v)| (edge_key(u, v), f as u32))
            })
            .collect();
        pairs.par_sort_unstable();
        let mut keys = Vec::new();
        let mut starts = Vec::new();
        for (i, run) in pairs.chunk_by(|a, b| a.0 == b.0).scan(0usize, |at, run| {
            let i = *at;
            *at += run.len();
            Some((i, run))
        }) {
            keys.push(run[0].0);
            starts.push(i as u32);
        }
        starts.push(pairs.len() as u32);
        let faces = pairs.into_iter().map(|p| p.1).collect();
        Self {
            keys,
            starts,
            faces,
        }
    }

    fn of(&self, e: usize) -> &[u32] {
        &self.faces[self.starts[e] as usize..self.starts[e + 1] as usize]
    }

    pub(crate) fn around(&self, a: u32, b: u32) -> &[u32] {
        self.keys
            .binary_search(&edge_key(a, b))
            .map_or(&[], |e| self.of(e))
    }

    fn all(&self) -> impl Iterator<Item = &[u32]> {
        (0..self.keys.len()).map(|e| self.of(e))
    }
}

/// The overhang regions of `surface` for a self-support angle (deg from
/// horizontal), footprints at `px_mm`, then its steep faces.
///
/// The overhang triangles of each connected part (parts joined through shared
/// edges, whether those faces overhang or not) are one region, in the order
/// of each part's first triangle; the steep faces follow.
pub fn overhang_regions(surface: &Surface<'_>, angle_deg: f32, px_mm: f32) -> Vec<OverhangRegion> {
    let n = surface.triangles.len();
    let Some(base_z) = base_z(surface) else {
        return Vec::new();
    };
    let limit = overhang_normal_z_limit(angle_deg);
    let normals: Vec<([f32; 3], f32)> = (0..n)
        .into_par_iter()
        .map(|f| surface.normal_and_area(f))
        .collect();
    let edges = EdgeFaces::new(surface.triangles);

    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            let up = parent[parent[i as usize] as usize];
            parent[i as usize] = up;
            i = up;
        }
        i
    }
    for faces in edges.all() {
        for pair in faces.windows(2) {
            let (a, b) = (root(&mut parent, pair[0]), root(&mut parent, pair[1]));
            if a != b {
                parent[a.max(b) as usize] = a.min(b);
            }
        }
    }
    // Each root is its part's first triangle: grouping in face order lists
    // the parts in that order.
    let mut group_of_root = vec![u32::MAX; n];
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for f in 0..n as u32 {
        if !is_overhang_normal_z(normals[f as usize].0[2], limit) {
            continue;
        }
        let r = root(&mut parent, f) as usize;
        if group_of_root[r] == u32::MAX {
            group_of_root[r] = groups.len() as u32;
            groups.push(Vec::new());
        }
        groups[group_of_root[r] as usize].push(f);
    }
    let mut regions: Vec<OverhangRegion> = groups
        .into_par_iter()
        .map(|faces| region(surface, &normals, faces, px_mm, base_z, false))
        .collect();
    regions.extend(steep_faces_with(
        surface, &normals, &edges, limit, px_mm, base_z,
    ));
    regions
}

/// The large flat faces between the self-support angle and 85°: a peel's
/// drag on one can lever a tall part over. A face grows from its lowest
/// numbered triangle through shared edges while each new triangle's normal
/// stays within 20° of the face's area-weighted mean so far (neighbours in
/// number order, nearest first), and counts from 150 mm².
pub fn steep_faces(surface: &Surface<'_>, angle_deg: f32, px_mm: f32) -> Vec<OverhangRegion> {
    let Some(base_z) = base_z(surface) else {
        return Vec::new();
    };
    let n = surface.triangles.len();
    let normals: Vec<([f32; 3], f32)> = (0..n)
        .into_par_iter()
        .map(|f| surface.normal_and_area(f))
        .collect();
    let edges = EdgeFaces::new(surface.triangles);
    steep_faces_with(
        surface,
        &normals,
        &edges,
        overhang_normal_z_limit(angle_deg),
        px_mm,
        base_z,
    )
}

fn steep_faces_with(
    surface: &Surface<'_>,
    normals: &[([f32; 3], f32)],
    edges: &EdgeFaces,
    limit: f32,
    px_mm: f32,
    base_z: f32,
) -> Vec<OverhangRegion> {
    let wall = -STEEP_FACE_MAX_ANGLE_DEG.to_radians().cos();
    let tolerance = STEEP_FACE_NORMAL_TOLERANCE_DEG.to_radians().cos();
    let steep = |f: u32| {
        let z = normals[f as usize].0[2];
        z >= limit && z <= wall
    };
    let mut taken = vec![false; surface.triangles.len()];
    let mut out = Vec::new();
    let mut next: Vec<u32> = Vec::new();
    for seed in 0..surface.triangles.len() as u32 {
        if taken[seed as usize] || !steep(seed) {
            continue;
        }
        taken[seed as usize] = true;
        let mut members = vec![seed];
        let (n0, a0) = normals[seed as usize];
        let mut sum = n0.map(|c| c * a0);
        let mut area = a0;
        // Breadth first: `members` is the queue, read from `head`.
        let mut head = 0;
        while head < members.len() {
            let f = members[head];
            head += 1;
            let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
            let mean = if length > 1e-9 {
                sum.map(|c| c / length)
            } else {
                [0.0, 0.0, -1.0]
            };
            let [a, b, c] = surface.triangles[f as usize];
            next.clear();
            for (u, v) in [(a, b), (b, c), (c, a)] {
                next.extend_from_slice(edges.around(u, v));
            }
            next.sort_unstable();
            next.dedup();
            for &g in &next {
                if g == f || taken[g as usize] || !steep(g) {
                    continue;
                }
                let (ng, ag) = normals[g as usize];
                if mean[0] * ng[0] + mean[1] * ng[1] + mean[2] * ng[2] < tolerance {
                    continue;
                }
                taken[g as usize] = true;
                members.push(g);
                area += ag;
                for k in 0..3 {
                    sum[k] += ng[k] * ag;
                }
            }
        }
        if area >= STEEP_FACE_MIN_AREA_MM2 {
            members.sort_unstable();
            out.push(region(surface, normals, members, px_mm, base_z, true));
        }
    }
    out
}

/// The part's own base: its lowest corner.
fn base_z(surface: &Surface<'_>) -> Option<f32> {
    let z = surface
        .positions
        .iter()
        .map(|p| p[2])
        .fold(f32::INFINITY, f32::min);
    (z.is_finite() && !surface.triangles.is_empty()).then_some(z)
}

fn region(
    surface: &Surface<'_>,
    normals: &[([f32; 3], f32)],
    faces: Vec<u32>,
    px_mm: f32,
    base_z: f32,
    steep_flat: bool,
) -> OverhangRegion {
    let mut area_mm2 = 0.0f32;
    let mut projected_area_mm2 = 0.0f32;
    let mut angle_sum = 0.0f32;
    let mut drag_moment_mm3 = 0.0f32;
    let mut sum = [0.0f32; 3];
    let (mut min_z, mut max_z) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut xy_min, mut xy_max) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for &f in &faces {
        let [a, b, c] = surface.corners(f as usize);
        let (n, area) = normals[f as usize];
        area_mm2 += area;
        for k in 0..3 {
            sum[k] += n[k] * area;
        }
        projected_area_mm2 +=
            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5;
        // Angle from horizontal: a face looking straight down is 0°.
        angle_sum += area * (-n[2].clamp(-1.0, 0.0)).acos().to_degrees();
        let sin = (n[0] * n[0] + n[1] * n[1]).sqrt();
        let height = (a[2] + b[2] + c[2]) / 3.0 - base_z;
        if sin > 1e-6 && height > 0.0 {
            drag_moment_mm3 += area * sin * height;
        }
        for p in [a, b, c] {
            min_z = min_z.min(p[2]);
            max_z = max_z.max(p[2]);
            for k in 0..2 {
                xy_min[k] = xy_min[k].min(p[k]);
                xy_max[k] = xy_max[k].max(p[k]);
            }
        }
    }
    let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
    let normal = if length > 1e-9 {
        sum.map(|c| c / length)
    } else {
        [0.0, 0.0, -1.0]
    };
    let footprint = footprint(surface, &faces, xy_min, xy_max, px_mm);
    let perimeter_loops = inset_outline(surface, &faces, OUTLINE_INSET_MM);
    OverhangRegion {
        area_mm2,
        projected_area_mm2,
        angle_deg: if area_mm2 > 1e-9 {
            angle_sum / area_mm2
        } else {
            0.0
        },
        normal,
        xy_min,
        xy_max,
        min_z,
        max_z,
        footprint,
        drag_moment_mm3,
        drag_dir_deg: if normal[0].abs() > 1e-9 || normal[1].abs() > 1e-9 {
            normal[1].atan2(normal[0]).to_degrees().rem_euclid(360.0)
        } else {
            0.0
        },
        steep_flat,
        perimeter_loops,
        triangle_ids: faces,
    }
}

/// The region's shadow as pixels whose centres start half a pixel outside
/// its box; a pixel takes the first of its triangles (in `faces` order)
/// whose shadow holds its centre, edges included.
fn footprint(
    surface: &Surface<'_>,
    faces: &[u32],
    xy_min: [f32; 2],
    xy_max: [f32; 2],
    px_mm: f32,
) -> FootprintMask {
    let px = px_mm.max(1e-4);
    let width = (((xy_max[0] - xy_min[0]) / px).ceil() as u32).max(1);
    let height = (((xy_max[1] - xy_min[1]) / px).ceil() as u32).max(1);
    let (origin_x, origin_y) = (xy_min[0] - px * 0.5, xy_min[1] - px * 0.5);
    let mut data = vec![0u8; (width * height) as usize];
    let mut surface_z = vec![0.0f32; data.len()];
    let pixel = |v: f32, from: f32| ((v - from) / px).floor() as i64;
    for &f in faces {
        let [a, b, c] = surface.corners(f as usize);
        let lo = |k: usize| a[k].min(b[k]).min(c[k]);
        let hi = |k: usize| a[k].max(b[k]).max(c[k]);
        let (x0, x1) = (
            pixel(lo(0), origin_x).max(0),
            pixel(hi(0), origin_x).min(width as i64 - 1),
        );
        let (y0, y1) = (
            pixel(lo(1), origin_y).max(0),
            pixel(hi(1), origin_y).min(height as i64 - 1),
        );
        for py in y0..=y1 {
            let y = origin_y + (py as f32 + 0.5) * px;
            for pxi in x0..=x1 {
                let i = (py as u32 * width + pxi as u32) as usize;
                if data[i] == 1 {
                    continue;
                }
                let x = origin_x + (pxi as f32 + 0.5) * px;
                if let Some(z) = height_over(x, y, a, b, c) {
                    data[i] = 1;
                    surface_z[i] = z;
                }
            }
        }
    }
    FootprintMask {
        width,
        height,
        origin_x,
        origin_y,
        px_mm: px,
        data,
        surface_z,
    }
}

/// The height of the triangle `abc` over `(x, y)`, when its shadow holds that
/// point (edges included).
fn height_over(x: f32, y: f32, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    let side = |p: [f32; 3], q: [f32; 3]| (x - q[0]) * (p[1] - q[1]) - (p[0] - q[0]) * (y - q[1]);
    let s = [side(a, b), side(b, c), side(c, a)];
    let below = s.iter().any(|&v| v < 0.0);
    let above = s.iter().any(|&v| v > 0.0);
    if below && above {
        return None;
    }
    let denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if denominator.abs() < 1e-9 {
        return Some((a[2] + b[2] + c[2]) / 3.0);
    }
    let wa = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / denominator;
    let wb = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / denominator;
    Some(wa * a[2] + wb * b[2] + (1.0 - wa - wb) * c[2])
}

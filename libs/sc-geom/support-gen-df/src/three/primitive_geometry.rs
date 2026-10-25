//! three.js r181 `CylinderGeometry` and `SphereGeometry` (MIT) at their
//! default angular ranges, positions and index only: the support
//! tessellation reads nothing else (normals and UVs are left out).

use super::buffer_geometry::BufferGeometry;
use super::math_utils::{js_cos, js_sin};

const TWO_PI: f64 = std::f64::consts::PI * 2.0;

/// `new CylinderGeometry(radiusTop, radiusBottom, height, radialSegments,
/// heightSegments, openEnded)` with `thetaStart = 0`, `thetaLength = 2π`.
pub fn cylinder_geometry(
    radius_top: f64,
    radius_bottom: f64,
    height: f64,
    radial_segments: f64,
    height_segments: f64,
    open_ended: bool,
) -> BufferGeometry {
    let radial = radial_segments.floor() as usize;
    let heights = height_segments.floor() as usize;
    let mut vertices: Vec<f64> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut index: u32 = 0;
    let half_height = height / 2.0;

    // Torso.
    let mut rows: Vec<Vec<u32>> = Vec::with_capacity(heights + 1);
    for y in 0..=heights {
        let mut row = Vec::with_capacity(radial + 1);
        let v = y as f64 / heights as f64;
        let radius = v * (radius_bottom - radius_top) + radius_top;
        for x in 0..=radial {
            let u = x as f64 / radial as f64;
            let theta = u * TWO_PI + 0.0;
            let (sin_theta, cos_theta) = (js_sin(theta), js_cos(theta));
            vertices.extend([
                radius * sin_theta,
                -v * height + half_height,
                radius * cos_theta,
            ]);
            row.push(index);
            index += 1;
        }
        rows.push(row);
    }
    for x in 0..radial {
        for y in 0..heights {
            let (a, b) = (rows[y][x], rows[y + 1][x]);
            let (c, d) = (rows[y + 1][x + 1], rows[y][x + 1]);
            if radius_top > 0.0 || y != 0 {
                indices.extend([a, b, d]);
            }
            if radius_bottom > 0.0 || y != heights - 1 {
                indices.extend([b, c, d]);
            }
        }
    }

    // Caps.
    let mut cap = |top: bool, vertices: &mut Vec<f64>, indices: &mut Vec<u32>| {
        let center_start = index;
        let radius = if top { radius_top } else { radius_bottom };
        let sign = if top { 1.0 } else { -1.0 };
        for _ in 1..=radial {
            vertices.extend([0.0, half_height * sign, 0.0]);
            index += 1;
        }
        let center_end = index;
        for x in 0..=radial {
            let u = x as f64 / radial as f64;
            let theta = u * TWO_PI + 0.0;
            let (cos_theta, sin_theta) = (js_cos(theta), js_sin(theta));
            vertices.extend([radius * sin_theta, half_height * sign, radius * cos_theta]);
            index += 1;
        }
        for x in 0..radial as u32 {
            let c = center_start + x;
            let i = center_end + x;
            if top {
                indices.extend([i, i + 1, c]);
            } else {
                indices.extend([i + 1, i, c]);
            }
        }
    };
    if !open_ended {
        if radius_top > 0.0 {
            cap(true, &mut vertices, &mut indices);
        }
        if radius_bottom > 0.0 {
            cap(false, &mut vertices, &mut indices);
        }
    }

    let mut g = BufferGeometry::default();
    g.set_position(&vertices);
    g.index = Some(indices);
    g
}

/// `new SphereGeometry(radius, widthSegments, heightSegments)` over the full
/// sphere.
pub fn sphere_geometry(radius: f64, width_segments: f64, height_segments: f64) -> BufferGeometry {
    let width = js_max_floor(3.0, width_segments);
    let height = js_max_floor(2.0, height_segments);
    let (phi_start, phi_length) = (0.0, TWO_PI);
    let (theta_start, theta_length) = (0.0, std::f64::consts::PI);
    let theta_end = (theta_start + theta_length).min(std::f64::consts::PI);

    let mut vertices: Vec<f64> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut grid: Vec<Vec<u32>> = Vec::with_capacity(height + 1);
    let mut index = 0u32;
    for iy in 0..=height {
        let mut row = Vec::with_capacity(width + 1);
        let v = iy as f64 / height as f64;
        for ix in 0..=width {
            let u = ix as f64 / width as f64;
            let phi = phi_start + u * phi_length;
            let theta = theta_start + v * theta_length;
            vertices.extend([
                -radius * js_cos(phi) * js_sin(theta),
                radius * js_cos(theta),
                radius * js_sin(phi) * js_sin(theta),
            ]);
            row.push(index);
            index += 1;
        }
        grid.push(row);
    }
    for iy in 0..height {
        for ix in 0..width {
            let (a, b) = (grid[iy][ix + 1], grid[iy][ix]);
            let (c, d) = (grid[iy + 1][ix], grid[iy + 1][ix + 1]);
            if iy != 0 || theta_start > 0.0 {
                indices.extend([a, b, d]);
            }
            if iy != height - 1 || theta_end < std::f64::consts::PI {
                indices.extend([b, c, d]);
            }
        }
    }
    let mut g = BufferGeometry::default();
    g.set_position(&vertices);
    g.index = Some(indices);
    g
}

/// `Math.max(min, Math.floor(segments))`.
fn js_max_floor(min: f64, segments: f64) -> usize {
    super::math_utils::js_max(min, segments.floor()) as usize
}

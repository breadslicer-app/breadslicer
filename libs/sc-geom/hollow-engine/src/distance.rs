//! Exact squared distances over the grid, one axis after another.

use rayon::prelude::*;

use crate::grid::GridSpec;

/// `field` (0 at the sources, infinite elsewhere) to each cell's squared
/// distance to the nearest source, in voxels: Felzenszwalb and
/// Huttenlocher's exact transform, one axis after another.
pub(crate) fn squared_distance_transform(grid: &GridSpec, field: &mut [f32]) {
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    // Along x: rows are contiguous.
    field.par_chunks_mut(nx).for_each(|row| {
        let line = lower_envelope(row);
        row.copy_from_slice(&line);
    });
    // Along y, then z: gathered per line.
    let plane = nx * ny;
    let lines_y: Vec<(usize, Vec<f32>)> = (0..nz * nx)
        .into_par_iter()
        .map(|k| {
            let (z, x) = (k / nx, k % nx);
            let line: Vec<f32> = (0..ny).map(|y| field[z * plane + y * nx + x]).collect();
            (k, lower_envelope(&line))
        })
        .collect();
    for (k, line) in lines_y {
        let (z, x) = (k / nx, k % nx);
        for (y, v) in line.into_iter().enumerate() {
            field[z * plane + y * nx + x] = v;
        }
    }
    let lines_z: Vec<(usize, Vec<f32>)> = (0..plane)
        .into_par_iter()
        .map(|k| {
            let line: Vec<f32> = (0..nz).map(|z| field[z * plane + k]).collect();
            (k, lower_envelope(&line))
        })
        .collect();
    for (k, line) in lines_z {
        for (z, v) in line.into_iter().enumerate() {
            field[z * plane + k] = v;
        }
    }
}

#[cfg(test)]
pub(crate) fn squared_distance_transform_for_test(
    nx: usize,
    ny: usize,
    nz: usize,
    field: &mut [f32],
) {
    let grid = GridSpec {
        nx,
        ny,
        nz,
        voxel_mm: 1.0,
        min: crate::mesh::Vec3::ZERO,
    };
    squared_distance_transform(&grid, field);
}

/// One axis of [`squared_distance_transform`]: `d[q] = min_p (q - p)² + f[p]`.
fn lower_envelope(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    let mut d = vec![f32::INFINITY; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0f32; n + 1];
    let Some(first) = f.iter().position(|x| x.is_finite()) else {
        return d;
    };
    let mut k = 0usize;
    v[0] = first;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in first + 1..n {
        if !f[q].is_finite() {
            continue;
        }
        let (qf, fq) = (q as f32, f[q]);
        loop {
            let p = v[k];
            let (pf, fp) = (p as f32, f[p]);
            let s = ((fq + qf * qf) - (fp + pf * pf)) / (2.0 * (qf - pf));
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // The first parabola is beaten everywhere.
                v[0] = q;
                z[1] = f32::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    let mut j = 0usize;
    for (q, out) in d.iter_mut().enumerate() {
        let qf = q as f32;
        while z[j + 1] < qf {
            j += 1;
        }
        let p = v[j];
        *out = (qf - p as f32) * (qf - p as f32) + f[p];
    }
    d
}

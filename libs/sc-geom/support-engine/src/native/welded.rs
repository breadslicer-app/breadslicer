//! A triangle soup welded into an indexed mesh, for the scans.

use std::collections::HashMap;

/// A soup's corners shared out as vertices: corners on the same grid point
/// share one, numbered in the order the soup first meets them.
#[derive(Debug, Clone, Default)]
pub struct WeldedMesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl WeldedMesh {
    /// `soup` (nine floats a triangle) welded on a grid `epsilon` times its
    /// bounding box's diagonal (that at least 1e-6 mm, the grid at least
    /// 1e-7 mm).
    pub fn weld(soup: &[f32], epsilon: f32) -> Self {
        let points = soup.as_chunks::<3>().0;
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in points {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let diagonal = if lo[0] > hi[0] {
            0.0
        } else {
            let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        };
        let per_step = 1.0 / (epsilon * diagonal.max(1e-6)).max(1e-7);
        let count = soup.len() / 9;
        let mut mesh = WeldedMesh {
            positions: Vec::with_capacity(count * 3 / 2),
            triangles: Vec::with_capacity(count),
        };
        let mut vertex_at: HashMap<[i32; 3], u32> = HashMap::with_capacity(count * 2);
        for corners in points[..count * 3].as_chunks::<3>().0 {
            let triangle = corners.map(|p| {
                let key = p.map(|v| (v * per_step).round() as i32);
                *vertex_at.entry(key).or_insert_with(|| {
                    mesh.positions.push(p);
                    mesh.positions.len() as u32 - 1
                })
            });
            mesh.triangles.push(triangle);
        }
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corners on the same grid point share a vertex, numbered as first met.
    #[test]
    fn shared_corners_weld_in_order() {
        let soup = [
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0, //
            1.0,
            0.0,
            0.0,
            1.0,
            1.0,
            0.0,
            0.0,
            1.0 + 1e-9,
            0.0,
        ];
        let mesh = WeldedMesh::weld(&soup, 1e-5);
        assert_eq!(mesh.positions.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [1, 3, 2]]);
    }
}

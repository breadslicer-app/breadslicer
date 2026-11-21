//! A region drawn on screen that the hollow keeps solid (#179).
//!
//! The person draws a closed curve over the model; what lies behind it, all
//! the way through the model as the camera saw it, stays material. The
//! region is kept as the camera's projection, not as voxels, so it holds when
//! the voxel size changes, the model moves, or the project reopens: a prism
//! along the view for an orthographic camera, a cone from the eye for a
//! perspective one, whose cross-section is the drawn polygon.
//!
//! The cavity takes the region's signed distance where it crosses it, so its
//! wall lands on the region's surface rather than on the voxels inside it.

use crate::mesh::Vec3;

/// A drawn region, in the frame of the points it is asked about.
///
/// A point `p` is written `p = origin + a·u + b·v + c·w`. An orthographic
/// region holds `p` when `(a, b)` is inside `polygon`, whatever `c`; a
/// perspective one when `c > 0` and `(a / c, b / c)` is inside it. `u`, `v`
/// and `w` need not be unit or square to each other (a scaled model's frame
/// shears them), which keeps the region exact under any affine map: map
/// `origin` as a point and the three as directions.
#[derive(Debug, Clone, PartialEq)]
pub struct LassoRegion {
    pub perspective: bool,
    pub origin: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub w: [f32; 3],
    /// The drawn curve, closed (the last point joins the first).
    pub polygon: Vec<[f32; 2]>,
}

impl LassoRegion {
    /// The region ready to be asked about many points; `None` when its axes
    /// are degenerate or its polygon has no area.
    pub fn prepared(&self) -> Option<PreparedLasso> {
        if self.polygon.len() < 3 {
            return None;
        }
        let [u, v, w] = [self.u, self.v, self.w].map(|a| Vec3::new(a[0], a[1], a[2]));
        let det = u.dot(v.cross(w));
        let scale = u.length().max(v.length());
        if !det.is_finite() || det.abs() <= 1e-12 * scale.powi(3).max(1e-30) {
            return None;
        }
        // Rows of the inverse of the matrix whose columns are u, v, w.
        let rows = [v.cross(w), w.cross(u), u.cross(v)].map(|r| r.scale(1.0 / det));
        let mut lo = [f32::MAX; 2];
        let mut hi = [f32::MIN; 2];
        for p in &self.polygon {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        Some(PreparedLasso {
            perspective: self.perspective,
            origin: Vec3::new(self.origin[0], self.origin[1], self.origin[2]),
            rows,
            scale,
            polygon: self.polygon.clone(),
            lo,
            hi,
        })
    }
}

/// A [`LassoRegion`] with its frame inverted, for asking about many points.
#[derive(Debug, Clone)]
pub struct PreparedLasso {
    perspective: bool,
    origin: Vec3,
    rows: [Vec3; 3],
    /// Millimetres per unit of the polygon's plane (at depth 1 for a cone).
    scale: f32,
    polygon: Vec<[f32; 2]>,
    lo: [f32; 2],
    hi: [f32; 2],
}

impl PreparedLasso {
    /// Where `p` projects in the polygon's plane, and how many millimetres
    /// one unit of that plane spans at `p`; `None` behind a cone's eye.
    fn project(&self, p: Vec3) -> Option<([f32; 2], f32)> {
        let q = p.sub(self.origin);
        let [a, b, c] = self.rows.map(|r| r.dot(q));
        if self.perspective {
            (c > 1e-9).then(|| ([a / c, b / c], self.scale * c))
        } else {
            Some(([a, b], self.scale))
        }
    }

    /// Whether the region holds `p`.
    pub fn contains(&self, p: Vec3) -> bool {
        let Some((x, _)) = self.project(p) else {
            return false;
        };
        if x[0] < self.lo[0] || x[0] > self.hi[0] || x[1] < self.lo[1] || x[1] > self.hi[1] {
            return false;
        }
        inside(&self.polygon, x)
    }

    /// The distance from `p` to the region's surface in millimetres,
    /// positive inside: exact on the surface itself, and to first order near
    /// it (a cone's sides spread with depth).
    pub fn signed_distance_mm(&self, p: Vec3) -> f32 {
        let Some((x, mm)) = self.project(p) else {
            return -f32::MAX;
        };
        let d = distance_to_outline(&self.polygon, x) * mm;
        if inside(&self.polygon, x) { d } else { -d }
    }
}

/// Even-odd point-in-polygon.
fn inside(polygon: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// The distance from `p` to the polygon's outline.
fn distance_to_outline(polygon: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let mut best = f32::MAX;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[j], polygon[i]);
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let (px, py) = (p[0] - a[0], p[1] - a[1]);
        let len2 = ex * ex + ey * ey;
        let t = if len2 > 0.0 {
            ((px * ex + py * ey) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (dx, dy) = (px - t * ex, py - t * ey);
        best = best.min(dx * dx + dy * dy);
        j = i;
    }
    best.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(perspective: bool) -> LassoRegion {
        LassoRegion {
            perspective,
            origin: [0.0, 0.0, -10.0],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            w: [0.0, 0.0, 1.0],
            polygon: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
        }
    }

    #[test]
    fn an_orthographic_region_is_a_prism_along_the_view() {
        let r = square(false).prepared().expect("prepared");
        for z in [-30.0, 0.0, 50.0] {
            assert!(r.contains(Vec3::new(0.5, -0.5, z)));
            assert!(!r.contains(Vec3::new(1.5, 0.0, z)));
            assert!((r.signed_distance_mm(Vec3::new(0.5, 0.0, z)) - 0.5).abs() < 1e-6);
            assert!((r.signed_distance_mm(Vec3::new(3.0, 0.0, z)) + 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn a_perspective_region_is_a_cone_from_the_eye() {
        let r = square(true).prepared().expect("prepared");
        // 10 mm in front of the eye the square spans ±10 mm, 20 mm in ±20.
        assert!(r.contains(Vec3::new(9.0, 0.0, 0.0)));
        assert!(!r.contains(Vec3::new(11.0, 0.0, 0.0)));
        assert!(r.contains(Vec3::new(19.0, 0.0, 10.0)));
        // Behind the eye: nothing.
        assert!(!r.contains(Vec3::new(0.0, 0.0, -11.0)));
        // On the side of the cone the distance is 0, and it grows with depth.
        assert!(r.signed_distance_mm(Vec3::new(10.0, 0.0, 0.0)).abs() < 1e-5);
        let near = r.signed_distance_mm(Vec3::new(9.0, 0.0, 0.0));
        let far = r.signed_distance_mm(Vec3::new(18.0, 0.0, 10.0));
        assert!((far - 2.0 * near).abs() < 1e-4, "{near} {far}");
    }

    #[test]
    fn a_region_holds_under_an_affine_map() {
        // A shear and a stretch: the same points stay in, the same out.
        let r = square(false);
        let map = |p: [f32; 3]| [2.0 * p[0] + 0.5 * p[1], p[1], 3.0 * p[2] + p[0]];
        let lin = |d: [f32; 3]| [2.0 * d[0] + 0.5 * d[1], d[1], 3.0 * d[2] + d[0]];
        let mapped = LassoRegion {
            origin: map(r.origin),
            u: lin(r.u),
            v: lin(r.v),
            w: lin(r.w),
            ..r.clone()
        };
        let (a, b) = (r.prepared().unwrap(), mapped.prepared().unwrap());
        for p in [
            [0.5, 0.2, 3.0],
            [1.2, 0.0, -4.0],
            [-0.9, 0.9, 7.0],
            [0.0, 1.1, 0.0],
        ] {
            let q = map(p);
            assert_eq!(
                a.contains(Vec3::new(p[0], p[1], p[2])),
                b.contains(Vec3::new(q[0], q[1], q[2]))
            );
        }
    }

    #[test]
    fn a_degenerate_region_is_refused() {
        let mut r = square(false);
        r.w = r.u;
        assert!(r.prepared().is_none());
        let mut r = square(false);
        r.polygon.truncate(2);
        assert!(r.prepared().is_none());
    }
}

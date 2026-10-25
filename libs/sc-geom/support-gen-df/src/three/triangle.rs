//! three.js r181 `src/math/Triangle.js` (MIT), the subset the support code calls.

use super::vector3::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Triangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
}

impl Triangle {
    pub const fn new(a: Vec3, b: Vec3, c: Vec3) -> Self {
        Triangle { a, b, c }
    }

    /// The point of the triangle closest to `p`.
    ///
    /// algorithm thanks to Real-Time Collision Detection by Christer Ericson,
    /// published by Morgan Kaufmann Publishers, (c) 2005 Elsevier Inc.,
    /// under the accompanying license; see chapter 5.1.5 for detailed explanation.
    /// basically, we're distinguishing which of the voronoi regions of the triangle
    /// the point lies in with the minimum amount of redundant computation.
    pub fn closest_point_to_point(&self, p: Vec3) -> Vec3 {
        let (a, b, c) = (self.a, self.b, self.c);

        let vab = Vec3::sub_vectors(b, a);
        let vac = Vec3::sub_vectors(c, a);
        let vap = Vec3::sub_vectors(p, a);
        let d1 = vab.dot(vap);
        let d2 = vac.dot(vap);
        if d1 <= 0.0 && d2 <= 0.0 {
            // vertex region of A; barycentric coords (1, 0, 0)
            return a;
        }

        let vbp = Vec3::sub_vectors(p, b);
        let d3 = vab.dot(vbp);
        let d4 = vac.dot(vbp);
        if d3 >= 0.0 && d4 <= d3 {
            // vertex region of B; barycentric coords (0, 1, 0)
            return b;
        }

        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            let v = d1 / (d1 - d3);
            // edge region of AB; barycentric coords (1-v, v, 0)
            return a.add_scaled_vector(vab, v);
        }

        let vcp = Vec3::sub_vectors(p, c);
        let d5 = vab.dot(vcp);
        let d6 = vac.dot(vcp);
        if d6 >= 0.0 && d5 <= d6 {
            // vertex region of C; barycentric coords (0, 0, 1)
            return c;
        }

        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            let w = d2 / (d2 - d6);
            // edge region of AC; barycentric coords (1-w, 0, w)
            return a.add_scaled_vector(vac, w);
        }

        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            let vbc = Vec3::sub_vectors(c, b);
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            // edge region of BC; barycentric coords (0, 1-w, w)
            return b.add_scaled_vector(vbc, w);
        }

        // face region
        let denom = 1.0 / (va + vb + vc);
        // u = va * denom
        let v = vb * denom;
        let w = vc * denom;

        a.add_scaled_vector(vab, v).add_scaled_vector(vac, w)
    }
}

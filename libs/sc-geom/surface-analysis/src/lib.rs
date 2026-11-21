//! How a posed model's surface faces the plate, read once for the support
//! stages and the live overhang map.
//!
//! - [`overhang_regions`]: the faces that look down more steeply than the
//!   model can print on its own, grouped per connected part, with what the
//!   placement reads from each group (areas, mean normal, a raster of its
//!   shadow with the surface height, its outline drawn in a little, the
//!   peel drag it carries); and the large flat faces just past that angle
//!   that a peel can lever the part over on ([`steep_faces`]).
//! - [`stability`]: how hard the peel's sideways drag pulls the whole part
//!   and how much its contact with the plate holds against it.
//!
//! A face is an overhang when it is flatter than the self-support angle
//! measured from horizontal: its unit normal's `z` is below
//! `-cos(angle)` ([`overhang_normal_z_limit`]).

mod outline;
mod regions;
mod stability;

pub use regions::{FootprintMask, OverhangRegion, overhang_regions, steep_faces};
pub use stability::{StabilityReport, stability};

/// A triangle mesh in millimetres, posed as it prints (plate below, `+z`
/// up), its triangles facing out.
#[derive(Debug, Clone, Copy)]
pub struct Surface<'a> {
    pub positions: &'a [[f32; 3]],
    pub triangles: &'a [[u32; 3]],
}

impl Surface<'_> {
    pub(crate) fn corners(&self, face: usize) -> [[f32; 3]; 3] {
        self.triangles[face].map(|i| self.positions[i as usize])
    }

    /// The face's unit normal and its area.
    pub(crate) fn normal_and_area(&self, face: usize) -> ([f32; 3], f32) {
        let [a, b, c] = self.corners(face);
        let u = sub(b, a);
        let v = sub(c, a);
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if length <= 0.0 {
            return ([0.0, 0.0, 0.0], 0.0);
        }
        (n.map(|c| c / length), length * 0.5)
    }
}

pub(crate) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The normal `z` below which a face is an overhang, for a self-support
/// angle from horizontal: a face at angle `θ` looking down has normal `z =
/// -cos θ`. The overhang map in the viewport reads the same limit.
pub fn overhang_normal_z_limit(self_support_angle_deg: f32) -> f32 {
    -self_support_angle_deg.to_radians().cos()
}

/// Whether a face whose unit normal has this `z` is an overhang under
/// `limit` ([`overhang_normal_z_limit`]).
pub fn is_overhang_normal_z(normal_z: f32, limit: f32) -> bool {
    normal_z < limit
}

#[cfg(test)]
mod tests;

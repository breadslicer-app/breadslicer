//! Where a posed model starts in the air, for support placement.
//!
//! The model is cut into layers, each a mask of pixels ([`layers`]). An
//! island is born where a layer grows past the one below by more than the
//! support buffer and that growth does not touch the growth below
//! ([`births`]); its contact is the centre of its first pixels, put on the
//! surface. Each island's body (#360) says how much model hangs on it before
//! it joins the rest ([`body`]). Separately, the surface's lowest points
//! (vertices lower than everything around them) are found from the mesh
//! itself.

pub mod births;
pub mod body;
pub mod layers;
pub mod mask;
mod surface;

use rayon::prelude::*;

pub use body::{BODY_CELL_MM, IslandBody, IslandBodyScan, MaskFrame};
pub use surface::LowPoint;

/// What a scan is asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanSettings {
    pub layer_height_mm: f64,
    pub px_mm: f64,
    /// How far a layer may grow past the one below and still stand on it.
    pub support_buffer_mm: f64,
    /// Pixels touching at a corner are one piece.
    pub corners: bool,
    /// How many rings of neighbours a lowest point is lower than.
    pub rings: usize,
    /// Measure each island's body.
    pub bodies: Option<IslandBodyScan>,
}

/// An island, where it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Island {
    /// Its contact, on the surface.
    pub contact: [f32; 3],
    /// The height of its first layer.
    pub base_z: f64,
    /// The area of its first layer (mm²).
    pub area_mm2: f32,
    /// Its first and last layers.
    pub layer_span: [u32; 2],
    pub body: Option<IslandBody>,
}

/// A scan's islands, in the order they are born, and the surface's lowest
/// points.
pub fn scan(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    s: &ScanSettings,
) -> (Vec<Island>, Vec<LowPoint>) {
    if positions.is_empty() || triangles.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let [min, max] = [lo, hi].map(|v| v.map(f64::from));
    let px = s.px_mm;
    let grid = layers::Grid {
        origin_x: min[0],
        top_y: max[1],
        px_mm: px,
        width: ((max[0] - min[0]) / px).ceil().max(1.0) as i32,
        height: ((max[1] - min[1]) / px).ceil().max(1.0) as i32,
    };
    let count = ((max[2] - min[2]) / s.layer_height_mm).ceil().max(0.0) as usize;
    let plan = surface::Plan::new(positions, triangles);
    let ((births, bodies), low) = rayon::join(
        || {
            let masks =
                layers::layer_masks(positions, triangles, grid, min[2], s.layer_height_mm, count);
            let buffer = (s.support_buffer_mm / px).round().max(0.0) as i32;
            let births = births::births(&masks, buffer, s.corners);
            let frame = MaskFrame {
                origin_x: grid.origin_x,
                top_y: grid.top_y,
                px_mm: px,
            };
            let bodies = match s.bodies {
                Some(request) => body::bodies(&masks, &births, s.layer_height_mm, frame, request),
                None => vec![None; births.len()],
            };
            (births, bodies)
        },
        || surface::low_points(positions, triangles, s.rings, &plan),
    );
    let reach = (s.layer_height_mm * 1.5) as f32;
    let islands = births
        .par_iter()
        .zip(bodies)
        .filter(|(b, _)| b.component.area as f64 * px * px >= 1e-4)
        .map(|(b, body)| {
            let (cx, cy) = b.component.centroid();
            let x = (grid.origin_x + cx * px + px * 0.5) as f32;
            let y = (grid.top_y - cy * px) as f32;
            let z = (min[2] + f64::from(b.layer) * s.layer_height_mm) as f32;
            Island {
                contact: surface::snapped(&plan, x, y, z, reach),
                base_z: f64::from(z),
                area_mm2: (b.component.area as f64 * px * px) as f32,
                layer_span: [b.layer, b.last_layer],
                body,
            }
        })
        .collect();
    (islands, low)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box floating over another: two islands, the lower box's base and the
    /// upper box's underside, each on its surface.
    #[test]
    fn a_box_over_a_box_is_two_islands() {
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for (lo, hi) in [
            ([0.0, 0.0, 0.0], [4.0, 4.0, 2.0]),
            ([0.0, 0.0, 3.0], [4.0, 4.0, 5.0]),
        ] {
            let base = positions.len() as u32;
            for i in 0..8usize {
                positions.push([
                    if i & 1 == 0 { lo[0] } else { hi[0] },
                    if i & 2 == 0 { lo[1] } else { hi[1] },
                    if i & 4 == 0 { lo[2] } else { hi[2] },
                ]);
            }
            for q in [
                [0, 2, 3, 1],
                [4, 5, 7, 6],
                [0, 1, 5, 4],
                [2, 6, 7, 3],
                [0, 4, 6, 2],
                [1, 3, 7, 5],
            ] {
                triangles.push([base + q[0], base + q[1], base + q[2]]);
                triangles.push([base + q[0], base + q[2], base + q[3]]);
            }
        }
        let settings = ScanSettings {
            layer_height_mm: 0.05,
            px_mm: 0.1,
            support_buffer_mm: 0.1,
            corners: false,
            rings: 2,
            bodies: None,
        };
        let (islands, low) = scan(&positions, &triangles, &settings);
        assert_eq!(islands.len(), 2, "{islands:?}");
        assert!((islands[1].contact[2] - 3.0).abs() < 1e-3, "{islands:?}");
        assert!((islands[1].area_mm2 - 16.0).abs() < 0.5);
        // A box's corners are no lower than all their neighbours.
        assert!(low.is_empty());
    }
}

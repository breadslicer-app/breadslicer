//! Layer islands and low points that are one place: each layer island takes
//! the nearest low point not yet taken within 0.5 mm on the plate and a
//! layer in height, and stands where it is.

use super::types::{DetectedIsland, IslandClass};

/// How near a low point has to be to a layer island to be the same place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntersectionOptions {
    pub xy_tolerance_mm: f64,
    pub z_band_mm: f64,
}

/// The layer islands, then the low points, each marked with whether it met
/// the other.
pub fn classify_intersection(
    voxel: &[DetectedIsland],
    minima: &[DetectedIsland],
    opts: IntersectionOptions,
) -> Vec<DetectedIsland> {
    let mut layers: Vec<DetectedIsland> = voxel.to_vec();
    let mut lows: Vec<DetectedIsland> = minima.to_vec();
    let reach2 = opts.xy_tolerance_mm * opts.xy_tolerance_mm;
    let mut taken = vec![false; lows.len()];
    for island in &mut layers {
        let mut best: Option<usize> = None;
        let mut best_d2 = f64::INFINITY;
        for (j, low) in lows.iter().enumerate() {
            if taken[j] {
                continue;
            }
            let dz = island.base_z - low.base_z;
            if dz.abs() > opts.z_band_mm {
                continue;
            }
            let (dx, dy) = (
                island.contact.x - low.contact.x,
                island.contact.y - low.contact.y,
            );
            let across2 = dx * dx + dy * dy;
            if across2 > reach2 {
                continue;
            }
            let d2 = across2 + dz * dz;
            if d2 < best_d2 {
                best_d2 = d2;
                best = Some(j);
            }
        }
        match best {
            Some(j) => {
                taken[j] = true;
                island.class = Some(IslandClass::Intersection);
                lows[j].class = Some(IslandClass::Intersection);
                island.contact = lows[j].contact;
            }
            None => island.class = Some(IslandClass::VoxelOnly),
        }
    }
    for (low, &taken) in lows.iter_mut().zip(&taken) {
        if !taken {
            low.class = Some(IslandClass::MinimaOnly);
        }
    }
    layers.extend(lows);
    layers
}

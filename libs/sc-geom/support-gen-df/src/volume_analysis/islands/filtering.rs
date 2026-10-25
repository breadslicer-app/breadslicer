//! DragonFruit `src/volumeAnalysis/Islands/filtering.ts`: the `supported` and
//! `grounded` flags, and the default-on filter that hides both.

use serde::{Deserialize, Serialize};

use super::spatial_hash_grid_2d::SpatialHashGrid2D;
use super::types::{DetectedIsland, SUPPORTED_RADIUS_MM};
use crate::supports::types::Vec3;

/// An existing support tip: world position and contact diameter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TipInfo {
    pub pos: Vec3,
    pub diameter_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandFilterInput<'a> {
    pub support_tips: &'a [TipInfo],
    pub plate_z: f64,
    pub supported_radius_mm: Option<f64>,
    pub grounded_epsilon_mm: Option<f64>,
    pub layer_height_mm: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandFilterToggles {
    pub show_already_supported: bool,
    pub show_plate_contact: bool,
}

pub const DEFAULT_FILTER_TOGGLES: IslandFilterToggles = IslandFilterToggles {
    show_already_supported: false,
    show_plate_contact: false,
};

/// `annotateFilterFlags`. A patch that declares a top (`maxZ`) is grounded only
/// when that top is at the plate too.
pub fn annotate_filter_flags(islands: &mut [DetectedIsland], input: &IslandFilterInput<'_>) {
    let grounded_eps = input.grounded_epsilon_mm.unwrap_or(1e-3);
    let plate_ceil = input.plate_z + grounded_eps;
    let layer_height = input.layer_height_mm.unwrap_or(0.25);
    let z_tolerance = 2.0 * layer_height;

    let mut grid: SpatialHashGrid2D<TipInfo> = SpatialHashGrid2D::new(1.0);
    let mut max_radius = input.supported_radius_mm.unwrap_or(SUPPORTED_RADIUS_MM);
    for tip in input.support_tips {
        grid.insert(tip.pos.x, tip.pos.y, *tip);
        let actual_radius = tip.diameter_mm / 2.0 + 0.15;
        if actual_radius > max_radius {
            max_radius = actual_radius;
        }
    }

    for island in islands {
        let top_z = island
            .max_z
            .filter(|z| z.is_finite())
            .unwrap_or(island.contact.z);
        island.grounded = Some(island.contact.z <= plate_ceil && top_z <= plate_ceil + z_tolerance);
        let supported = grid
            .query(island.contact.x, island.contact.y, max_radius)
            .iter()
            .any(|tip| {
                let actual_radius = tip.diameter_mm / 2.0 + 0.15;
                let dz = (tip.pos.z - island.contact.z).abs();
                if dz <= z_tolerance {
                    let dx = tip.pos.x - island.contact.x;
                    let dy = tip.pos.y - island.contact.y;
                    dx * dx + dy * dy <= actual_radius * actual_radius
                } else {
                    false
                }
            });
        island.supported = Some(supported);
    }
}

/// `applyFilter`: hide supported and plate-contact islands unless toggled on.
pub fn apply_filter(
    islands: Vec<DetectedIsland>,
    toggles: IslandFilterToggles,
) -> Vec<DetectedIsland> {
    islands
        .into_iter()
        .filter(|i| {
            !(i.supported == Some(true) && !toggles.show_already_supported)
                && !(i.grounded == Some(true) && !toggles.show_plate_contact)
        })
        .collect()
}

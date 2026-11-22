//! A model's islands from its scans: the layer islands big enough, the low
//! points of its surface, the two met where they are one place, and the
//! overhang regions; none that stands on the plate.
//!
//! The scans measure in `f32`; each value is read as its shortest decimal
//! ([`ipc32`]), the number the islands have always carried.

use super::intersection::{IntersectionOptions, classify_intersection};
use super::types::{DetectedIsland, IslandSource};
use super::voxel_footprint::VoxelFootprintBuilder;
use crate::native::mesh_minima::{CombinedIslandScanResult, ScanPoint};
use crate::native::overhang::{OverhangRegion, OverhangScan};
use support_model::types::Vec3;

/// An `f32` as its shortest decimal, read back as an `f64`; NaN for one not
/// finite.
pub fn ipc32(v: f32) -> f64 {
    if v.is_finite() {
        serde_json::to_string(&v)
            .expect("a finite f32 is written")
            .parse()
            .expect("its own text reads back")
    } else {
        f64::NAN
    }
}

fn point(p: ScanPoint) -> Vec3 {
    Vec3::new(ipc32(p.x), ipc32(p.y), ipc32(p.z))
}

/// An overhang region as an island: held at the lowest cell of its footprint
/// with a height (the middle of the footprint at its lowest height when none
/// has one).
pub fn overhang_region_to_island(region: &OverhangRegion, i: usize) -> DetectedIsland {
    let f = &region.footprint;
    let (width, height) = (f.width as usize, f.height as usize);
    let (origin_x, origin_y, px) = (ipc32(f.origin_x), ipc32(f.origin_y), ipc32(f.px_mm));
    let mut cells = VoxelFootprintBuilder::new(true);
    let mut lowest: Option<(usize, f64)> = None;
    for row in 0..height {
        for col in 0..width {
            let at = row * width + col;
            if f.data[at] == 0 {
                continue;
            }
            let x = origin_x + (col as f64 + 0.5) * px;
            let y = origin_y + (row as f64 + 0.5) * px;
            // A cell with no height is stored at 0 and never held.
            let z = f.surface_z[at];
            let z = z.is_finite().then(|| ipc32(z));
            cells.push(x, y, z);
            if let Some(z) = z
                && lowest.is_none_or(|(_, low)| z < low)
            {
                lowest = Some((at, z));
            }
        }
    }
    let contact = match lowest {
        Some((at, z)) => Vec3::new(
            origin_x + ((at % width) as f64 + 0.5) * px,
            origin_y + ((at / width) as f64 + 0.5) * px,
            z,
        ),
        None => Vec3::new(
            origin_x + (width as f64 * px) / 2.0,
            origin_y + (height as f64 * px) / 2.0,
            ipc32(region.min_z),
        ),
    };
    DetectedIsland {
        id: format!("o{i}"),
        source: IslandSource::Overhang,
        contact,
        base_z: contact.z,
        area_mm2: Some(ipc32(region.projected_area_mm2)),
        overhang_angle_deg: Some(ipc32(region.angle_deg)),
        max_z: Some(ipc32(region.max_z)),
        triangle_ids: Some(region.triangle_ids.iter().map(|&t| t as usize).collect()),
        surface_normal: Some(Vec3::new(
            ipc32(region.normal[0]),
            ipc32(region.normal[1]),
            ipc32(region.normal[2]),
        )),
        contact_voxels: Some(cells.build()),
        perimeter_loops: Some(
            region
                .perimeter_loops
                .iter()
                .map(|l| {
                    l.iter()
                        .map(|p| [ipc32(p[0]), ipc32(p[1]), ipc32(p[2])])
                        .collect()
                })
                .collect(),
        ),
        ..DetectedIsland::default()
    }
}

/// What the islands are read with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandSettings {
    /// Layer islands smaller than this are none, mm².
    pub min_area_mm2: f64,
    pub layer_height_mm: f64,
    /// The plate's height.
    pub plate_z: f64,
}

impl IslandSettings {
    pub fn defaults(layer_height_mm: f64) -> Self {
        IslandSettings {
            min_area_mm2: 0.02,
            layer_height_mm,
            plate_z: 0.0,
        }
    }
}

/// A model's islands.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedIslands {
    pub filtered: Vec<DetectedIsland>,
}

/// The islands of `combined` (layer islands and low points) and of
/// `overhangs` (its regions), as [`DerivedIslands`] says. One on the plate
/// (within 1 µm, an overhang with its top within two layers) is held by it.
pub fn derive_islands(
    combined: &CombinedIslandScanResult,
    overhangs: &OverhangScan,
    settings: &IslandSettings,
) -> DerivedIslands {
    let layers: Vec<DetectedIsland> = combined
        .voxel_islands
        .iter()
        .filter(|v| ipc32(v.area_mm2) >= settings.min_area_mm2)
        .map(|v| DetectedIsland {
            id: v.id.clone(),
            source: IslandSource::Voxel,
            contact: point(v.contact),
            base_z: v.base_z,
            area_mm2: Some(ipc32(v.area_mm2)),
            layer_span: Some([f64::from(v.layer_span[0]), f64::from(v.layer_span[1])]),
            body: v.body.clone().map(std::sync::Arc::new),
            ..DetectedIsland::default()
        })
        .collect();
    let lows: Vec<DetectedIsland> = combined
        .minima_islands
        .iter()
        .enumerate()
        .map(|(i, m)| DetectedIsland {
            id: format!("m{i}"),
            source: IslandSource::Minima,
            contact: point(m.position),
            base_z: ipc32(m.position.z),
            ..DetectedIsland::default()
        })
        .collect();
    let mut islands = classify_intersection(
        &layers,
        &lows,
        IntersectionOptions {
            xy_tolerance_mm: 0.5,
            z_band_mm: settings.layer_height_mm,
        },
    );
    islands.extend(
        overhangs
            .regions
            .iter()
            .enumerate()
            .map(|(i, r)| overhang_region_to_island(r, i)),
    );
    let plate = settings.plate_z + 1e-3;
    let two_layers = 2.0 * settings.layer_height_mm;
    islands.retain(|island| {
        let top = island
            .max_z
            .filter(|z| z.is_finite())
            .unwrap_or(island.contact.z);
        !(island.contact.z <= plate && top <= plate + two_layers)
    });
    DerivedIslands { filtered: islands }
}

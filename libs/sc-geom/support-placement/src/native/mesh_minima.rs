//! The island scan of a posed model, as the support stages read it: the
//! `islands` crate's births and lowest points on this engine's mesh, and,
//! when asked, its overhang regions.

use serde::{Deserialize, Serialize};

use super::islands::body::{IslandBody, IslandBodyScan};
use super::overhang;
use super::welded::WeldedMesh;

/// A point a scan found, mm.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq)]
pub struct ScanPoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A vertex lower than every vertex near it, whose surface faces down or
/// which stands outside the model.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LocalMinimum {
    pub vertex_index: u32,
    pub position: ScanPoint,
    pub seed_triangle_id: u32,
}

/// An island the layer scan found, where it starts.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VoxelIsland {
    pub id: String,
    pub source: String,
    pub contact: ScanPoint,
    pub base_z: f64,
    pub area_mm2: f32,
    pub layer_span: [u32; 2],
    /// Its body (#360), when the scan was asked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<IslandBody>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CombinedIslandScanResult {
    pub voxel_islands: Vec<VoxelIsland>,
    pub minima_islands: Vec<LocalMinimum>,
    pub overhang_islands: Vec<overhang::OverhangRegion>,
}

/// The island scan with the overhang regions too (at 45° and 0.25 mm unless
/// given).
#[allow(clippy::too_many_arguments)]
pub fn scan_islands(
    mesh: &WeldedMesh,
    layer_height_mm: f64,
    px_mm: f64,
    support_buffer_mm: f64,
    connectivity: u8,
    k: Option<usize>,
    overhang_self_support_angle_deg: Option<f32>,
    overhang_px_mm: Option<f32>,
) -> CombinedIslandScanResult {
    let mut scan = scan_islands_with_bodies(
        mesh,
        layer_height_mm,
        px_mm,
        support_buffer_mm,
        connectivity,
        k,
        None,
    );
    scan.overhang_islands = overhang::classify_overhangs(
        mesh,
        overhang_self_support_angle_deg.unwrap_or(45.0),
        overhang_px_mm.unwrap_or(0.25),
    );
    scan
}

/// The island scan without overhang regions.
pub fn scan_islands_without_overhangs(
    mesh: &WeldedMesh,
    layer_height_mm: f64,
    px_mm: f64,
    support_buffer_mm: f64,
    connectivity: u8,
    k: Option<usize>,
) -> CombinedIslandScanResult {
    scan_islands_with_bodies(
        mesh,
        layer_height_mm,
        px_mm,
        support_buffer_mm,
        connectivity,
        k,
        None,
    )
}

/// [`scan_islands_without_overhangs`] that also measures every island's
/// body when `bodies` asks for it (#360).
pub fn scan_islands_with_bodies(
    mesh: &WeldedMesh,
    layer_height_mm: f64,
    px_mm: f64,
    support_buffer_mm: f64,
    connectivity: u8,
    k: Option<usize>,
    bodies: Option<IslandBodyScan>,
) -> CombinedIslandScanResult {
    let settings = bread_islands::ScanSettings {
        layer_height_mm,
        px_mm,
        support_buffer_mm,
        corners: connectivity == 8,
        rings: k.unwrap_or(2),
        bodies,
    };
    let (found, low) = bread_islands::scan(&mesh.positions, &mesh.triangles, &settings);
    let v = |p: [f32; 3]| ScanPoint {
        x: p[0],
        y: p[1],
        z: p[2],
    };
    CombinedIslandScanResult {
        voxel_islands: found
            .into_iter()
            .enumerate()
            .map(|(i, island)| VoxelIsland {
                id: format!("v{i}"),
                source: "voxel".to_string(),
                contact: v(island.contact),
                base_z: island.base_z,
                area_mm2: island.area_mm2,
                layer_span: island.layer_span,
                body: island.body,
            })
            .collect(),
        minima_islands: low
            .into_iter()
            .map(|m| LocalMinimum {
                vertex_index: m.vertex,
                position: v(m.position),
                seed_triangle_id: m.face,
            })
            .collect(),
        overhang_islands: Vec::new(),
    }
}

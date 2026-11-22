//! The overhang scan of a posed model, as the support stages read it: the
//! `surface-analysis` crate's regions and stability report on this engine's
//! mesh.

use serde::{Deserialize, Serialize};

pub use surface_analysis::{
    FootprintMask, OverhangRegion, StabilityReport, is_overhang_normal_z, overhang_normal_z_limit,
};

use super::welded::WeldedMesh;

/// The weld [`overhang_and_stability_from_soup`] applies: a caller holding
/// the soup welded with this epsilon can pass that mesh to
/// [`overhang_and_stability`] instead.
pub const OVERHANG_WELD_EPSILON: f32 = 1e-5;

/// Welds a world-space soup (nine numbers per triangle) and scans it: the
/// overhang regions and the stability report, one weld for both.
pub fn overhang_and_stability_from_soup(
    positions: &[f32],
    self_support_angle_deg: f32,
    px_mm: f32,
    has_raft: bool,
) -> (Vec<OverhangRegion>, Option<StabilityReport>) {
    let mesh = {
        let _p = support_geom::perf::scope("ovh:weld");
        WeldedMesh::weld(positions, OVERHANG_WELD_EPSILON)
    };
    overhang_and_stability(&mesh, self_support_angle_deg, px_mm, has_raft)
}

/// [`overhang_and_stability_from_soup`] on a soup already welded with
/// [`OVERHANG_WELD_EPSILON`].
pub fn overhang_and_stability(
    mesh: &WeldedMesh,
    self_support_angle_deg: f32,
    px_mm: f32,
    has_raft: bool,
) -> (Vec<OverhangRegion>, Option<StabilityReport>) {
    let surface = surface_analysis::Surface {
        positions: &mesh.positions,
        triangles: &mesh.triangles,
    };
    let regions = {
        let _p = support_geom::perf::scope("ovh:regions");
        surface_analysis::overhang_regions(&surface, self_support_angle_deg, px_mm)
    };
    let report = {
        let _p = support_geom::perf::scope("ovh:stability");
        surface_analysis::stability(&surface, has_raft)
    };
    (regions, report)
}

/// The overhang regions (and steep faces) of an already welded mesh.
pub fn classify_overhangs(
    mesh: &WeldedMesh,
    self_support_angle_deg: f32,
    px_mm: f32,
) -> Vec<OverhangRegion> {
    let surface = surface_analysis::Surface {
        positions: &mesh.positions,
        triangles: &mesh.triangles,
    };
    surface_analysis::overhang_regions(&surface, self_support_angle_deg, px_mm)
}

/// One scan's answer: the regions, and the stability report for the pose
/// they were found in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverhangScan {
    pub regions: Vec<OverhangRegion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stability: Option<StabilityReport>,
    /// The down-facing regions up to a steeper angle, for the zone rules'
    /// stabilization (#360); empty unless asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steep_regions: Vec<OverhangRegion>,
}

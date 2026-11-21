//! DragonFruit's hole-punch interface (`punch_cylinders`), on the exact cut.
//!
//! Same specification as DragonFruit (`HolePunchSpec`: a start point
//! normalized to the source bounding box, which may lie outside it, an axis,
//! a length and an optionally oval radius), same cylinder segment count. The
//! cylinders are cut one after the other with [`subtract_convex`] instead of
//! as one manifold3d union, so a failure names the punch that caused it.

use crate::drain::{ConvexCutter, CutError, subtract_convex};
use crate::hollowing::hole_axis_and_length;
use crate::mesh::{IndexedMesh, Vec3};

#[derive(Debug, Clone, PartialEq)]
pub struct HolePunchSpec {
    /// Start of the cylinder, normalized to the source bbox (not clamped).
    pub center_norm: [f32; 3],
    /// Cylinder radius in millimeters (first axis).
    pub radius_mm: f32,
    /// Second radius for oval punches; defaults to `radius_mm`.
    pub radius_y_mm: Option<f32>,
    /// Unit direction of the punch axis, in source-mesh space.
    pub direction: Option<[f32; 3]>,
    /// Punch length in millimeters.
    pub length_mm: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HolePunchOptions {
    pub punches: Vec<HolePunchSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HolePunchReport {
    pub source_triangle_count: usize,
    pub output_triangle_count: usize,
    pub removed_triangle_count: usize,
    pub punch_count: usize,
}

#[derive(Debug, Clone)]
pub struct HolePunchOutcome {
    pub mesh: IndexedMesh,
    pub report: HolePunchReport,
}

/// A punch that could not be cut.
#[derive(Debug, Clone, PartialEq)]
pub struct PunchError {
    pub index: usize,
    pub error: CutError,
}

impl std::fmt::Display for PunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "punch {}: {}", self.index + 1, self.error)
    }
}

impl std::error::Error for PunchError {}

/// The prism DragonFruit's manifold path builds for a punch, or `None` when
/// it skips it (no radius, no length).
pub fn punch_cutter(bbox: &crate::mesh::Aabb, punch: &HolePunchSpec) -> Option<ConvexCutter> {
    if punch.radius_mm <= 0.0 {
        return None;
    }
    let center = Vec3::new(
        bbox.min.x + (bbox.max.x - bbox.min.x) * punch.center_norm[0],
        bbox.min.y + (bbox.max.y - bbox.min.y) * punch.center_norm[1],
        bbox.min.z + (bbox.max.z - bbox.min.z) * punch.center_norm[2],
    );
    let (axis, length_mm) = hole_axis_and_length(
        punch.direction,
        punch.center_norm,
        punch.length_mm,
        bbox,
        0.02,
    );
    if length_mm <= 1e-4 {
        return None;
    }
    let radius = punch.radius_mm.max(0.02);
    let radius_y = punch.radius_y_mm.unwrap_or(punch.radius_mm).max(0.02);
    let circumference = std::f32::consts::TAU * radius.max(radius_y);
    let segments = ((circumference / 0.7).ceil() as usize).clamp(16, 80);
    Some(ConvexCutter::prism(
        center, axis, radius, radius_y, length_mm, segments, 0.0,
    ))
}

/// Cuts every punch from `mesh`, which must be closed and edge-manifold.
pub fn punch_cylinders(
    mesh: IndexedMesh,
    options: &HolePunchOptions,
) -> Result<HolePunchOutcome, PunchError> {
    let source_triangle_count = mesh.triangle_count();
    let bbox = mesh.bbox();
    let mut out = mesh;
    if source_triangle_count > 0 {
        for (index, punch) in options.punches.iter().enumerate() {
            if let Some(cutter) = punch_cutter(&bbox, punch) {
                out =
                    subtract_convex(&out, &cutter).map_err(|error| PunchError { index, error })?;
            }
        }
    }
    let output_triangle_count = out.triangle_count();
    Ok(HolePunchOutcome {
        mesh: out,
        report: HolePunchReport {
            source_triangle_count,
            output_triangle_count,
            removed_triangle_count: source_triangle_count.saturating_sub(output_triangle_count),
            punch_count: options.punches.len(),
        },
    })
}

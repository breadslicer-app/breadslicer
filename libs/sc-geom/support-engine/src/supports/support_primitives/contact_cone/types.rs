//! DragonFruit `src/supports/SupportPrimitives/ContactCone/types.ts`.

use serde::{Deserialize, Serialize};

use crate::supports::types::{PlacementSurface, Vec3};

/// The `type: 'disk'` tag a contact disk profile carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DiskTag {
    #[default]
    #[serde(rename = "disk")]
    Disk,
}

/// ContactDiskProfile: "The Nib"
/// A flat cylinder with variable standoff thickness.
///
/// The numeric fields are declared required in DragonFruit but every reader
/// falls back (`?? DEFAULT`) for legacy profiles that lack them, so they are
/// optional here and the fallbacks stay where DragonFruit wrote them.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactDiskProfile {
    #[serde(rename = "type", default)]
    pub kind: DiskTag,
    /// Minimum base thickness (flat surfaces)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_thickness_mm: Option<f64>,
    /// Max extension length (steep surfaces)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_standoff_mm: Option<f64>,
    /// Angle (radians) where extension starts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standoff_angle_threshold: Option<f64>,
    /// Contact footprint diameter (the flat face against the model). The nib is
    /// also the round tip's socket — the ball is centered on the nib's top face —
    /// so the nib must be at least half this tall. Optional because the profile
    /// reaches `calculateDiskThickness` as a bare `ContactDiskProfile` in a few
    /// joints/twigs; when absent the nib keeps the pure angle-based thickness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_diameter_mm: Option<f64>,
}

/// Which primitive a tip carries: `ContactDiskProfile | ContactSphereProfile | { type?: undefined }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TipPrimitive {
    Disk,
    Sphere,
}

/// SupportTipProfile: Configuration for contact cone geometry.
/// A discriminated union on `type`; the variant fields are kept flat as on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportTipProfile {
    /// The actual contact footprint size
    pub contact_diameter_mm: f64,
    /// Larger end (socket side)
    pub body_diameter_mm: f64,
    /// Total head length (excluding primitive offset)
    pub length_mm: f64,
    /// Embed depth (legacy use, mostly for sphere now)
    pub penetration_mm: f64,
    /// Allow undefined for legacy compatibility
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TipPrimitive>,
    // ContactDiskProfile
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_thickness_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_standoff_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standoff_angle_threshold: Option<f64>,
    // ContactSphereProfile
    /// Multiplier of contact diameter (e.g. 1.5x)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sphere_radius_ratio: Option<f64>,
    /// SoulCrafted (#360): a neck of the contact diameter this long between
    /// the surface and the cone, where the support breaks off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_point_mm: Option<f64>,
}

impl SupportTipProfile {
    /// `profile.type === 'disk'`.
    #[inline]
    pub fn is_disk(&self) -> bool {
        self.kind == Some(TipPrimitive::Disk)
    }

    /// The tip read as the `ContactDiskProfile` DragonFruit passes it as.
    ///
    /// TypeScript hands the whole tip profile to `calculateDiskThickness`
    /// structurally, so the disk reader sees the tip's own `contactDiameterMm`.
    pub fn as_disk_profile(&self) -> ContactDiskProfile {
        ContactDiskProfile {
            kind: DiskTag::Disk,
            disk_thickness_mm: self.disk_thickness_mm,
            max_standoff_mm: self.max_standoff_mm,
            standoff_angle_threshold: self.standoff_angle_threshold,
            contact_diameter_mm: Some(self.contact_diameter_mm),
        }
    }
}

/// ContactCone: The terminal piece at the model interface.
/// - Contact face touches model
/// - Socket side connects directly to a Joint (never to a Shaft)
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCone {
    pub id: String,
    /// Contact point on model surface
    pub pos: Vec3,
    /// Cone axis direction (points into model)
    pub normal: Vec3,
    /// ACTUAL surface normal (for disk alignment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_normal: Option<Vec3>,
    /// Override for disk thickness (e.g. from collision)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_length_override: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_surface: Option<PlacementSurface>,
    pub profile: SupportTipProfile,
    /// The Joint this cone connects to (optional for Leaf)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_joint_id: Option<String>,
}

/// Default profile matching legacy behavior.
/// Defaults to Disk type for Phase 1.
pub const DEFAULT_TIP_PROFILE: SupportTipProfile = SupportTipProfile {
    kind: Some(TipPrimitive::Disk),
    contact_diameter_mm: 0.4,
    body_diameter_mm: 1.2,
    length_mm: 3.0,
    penetration_mm: 0.05,
    disk_thickness_mm: Some(0.1),
    // Reduced from 1.5mm to prevent fragility
    max_standoff_mm: Some(0.25),
    // 45 degrees
    standoff_angle_threshold: Some(std::f64::consts::PI / 4.0),
    sphere_radius_ratio: None,
    break_point_mm: None,
};

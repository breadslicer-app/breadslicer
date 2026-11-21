//! Tips: where a support meets the model. A tip is a cone from the contact
//! out to the shaft's socket; a disk tip first stands off the surface on a
//! short nib, longer the more the cone leans against the surface.

use serde::{Deserialize, Serialize};

use crate::geom::Quaternion;
use crate::supports::placement_logic::collision_avoidance::{
    CollisionField, CollisionFrustumProfile, calculate_safe_offset,
};
use crate::supports::types::{PlacementSurface, Vec3};

/// The `disk` tag a nib's profile carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DiskTag {
    #[default]
    #[serde(rename = "disk")]
    Disk,
}

/// A nib: how short it may be, how long, and past what angle between the
/// surface and the cone it grows. Missing values take the defaults of
/// [`nib_length`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactDiskProfile {
    #[serde(rename = "type", default)]
    pub kind: DiskTag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_thickness_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_standoff_mm: Option<f64>,
    /// Radians.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standoff_angle_threshold: Option<f64>,
    /// The flat face against the model. A round tip's ball sits on the nib's
    /// top face, so the nib is at least half this tall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_diameter_mm: Option<f64>,
}

/// What a tip starts with on the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TipPrimitive {
    Disk,
    Sphere,
}

/// A tip's sizes.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportTipProfile {
    /// Where it touches the model.
    pub contact_diameter_mm: f64,
    /// Where it meets the shaft.
    pub body_diameter_mm: f64,
    /// The cone's length, without the nib.
    pub length_mm: f64,
    /// How far it goes into the model.
    pub penetration_mm: f64,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TipPrimitive>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_thickness_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_standoff_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standoff_angle_threshold: Option<f64>,
    /// A neck of the contact's width this long between the surface and the
    /// cone, where the support snaps off (#360).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_point_mm: Option<f64>,
}

impl SupportTipProfile {
    #[inline]
    pub fn is_disk(&self) -> bool {
        self.kind == Some(TipPrimitive::Disk)
    }

    /// The tip's nib.
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

/// A tip on the model.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCone {
    pub id: String,
    /// The contact on the model's surface.
    pub pos: Vec3,
    /// The cone's axis, from the contact out to the socket.
    pub normal: Vec3,
    /// The surface's own normal at the contact, which the nib follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_normal: Option<Vec3>,
    /// The nib's length when it was set (to clear the model, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_length_override: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_surface: Option<PlacementSurface>,
    pub profile: SupportTipProfile,
    /// The joint its socket meets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_joint_id: Option<String>,
}

impl ContactCone {
    /// The surface's normal, or the axis when it has none.
    fn surface(&self) -> Vec3 {
        self.surface_normal.unwrap_or(self.normal)
    }

    /// The nib's length: what was set, or what the angle calls for; none
    /// for a tip without a nib.
    fn nib(&self) -> f64 {
        if !self.profile.is_disk() {
            return 0.0;
        }
        self.disk_length_override.unwrap_or_else(|| {
            nib_length(
                self.surface(),
                self.normal,
                Some(&self.profile.as_disk_profile()),
            )
        })
    }
}

/// The tip sizes a profile that says nothing gets.
pub const DEFAULT_TIP_PROFILE: SupportTipProfile = SupportTipProfile {
    kind: Some(TipPrimitive::Disk),
    contact_diameter_mm: 0.4,
    body_diameter_mm: 1.2,
    length_mm: 3.0,
    penetration_mm: 0.05,
    disk_thickness_mm: Some(0.1),
    max_standoff_mm: Some(0.25),
    standoff_angle_threshold: Some(std::f64::consts::FRAC_PI_4),
    break_point_mm: None,
};

/// The socket of a cone starting at `start` along `axis`, its profile's
/// length away.
pub fn get_socket_position(start: Vec3, axis: Vec3, profile: Option<&SupportTipProfile>) -> Vec3 {
    let length = profile.unwrap_or(&DEFAULT_TIP_PROFILE).length_mm;
    Vec3::new(
        start.x + axis.x * length,
        start.y + axis.y * length,
        start.z + axis.z * length,
    )
}

/// Where a tip's socket is: past its nib along the surface's normal, then
/// the cone's length along its axis.
pub fn get_final_socket_position(cone: &ContactCone) -> Vec3 {
    let (surface, nib) = (cone.surface(), cone.nib());
    let start = Vec3::new(
        cone.pos.x + surface.x * nib,
        cone.pos.y + surface.y * nib,
        cone.pos.z + surface.z * nib,
    );
    get_socket_position(start, cone.normal, Some(&cone.profile))
}

/// The lowest point of a tip: the rims of its cone's ends (a tilted end dips
/// below its centre) and, for a disk tip, the nib's back face.
pub fn get_contact_cone_lowest_z(cone: &ContactCone) -> f64 {
    let (surface, profile) = (cone.surface(), &cone.profile);
    let start = cone.pos.z + surface.z * cone.nib();
    let (r_contact, r_body) = (
        profile.contact_diameter_mm / 2.0,
        profile.body_diameter_mm / 2.0,
    );
    // How far a ring around an axis reaches down: the axis's sideways part.
    let dip = |axis_z: f64| (1.0 - axis_z * axis_z).max(0.0).sqrt();
    let cone_dip = dip(cone.normal.z);
    let mut lowest = (start - r_contact)
        .min(start - r_contact * cone_dip)
        .min(start + cone.normal.z * profile.length_mm - r_body * cone_dip);
    if profile.is_disk() {
        let back = cone.pos.z - surface.z * profile.penetration_mm.max(0.0);
        lowest = lowest.min(back.min(start) - r_contact * dip(surface.z));
    }
    lowest
}

/// A nib's least length when its profile says none.
const NIB_MIN_MM: f64 = 0.1;
/// The angle past which a nib grows, when its profile says none.
const NIB_GROWS_PAST_RAD: f64 = std::f64::consts::FRAC_PI_4;
/// The longest a nib grows when its profile says none; a profile that says
/// 1.5 (an old default) gets this too.
const NIB_MAX_MM: f64 = 0.35;
const OLD_NIB_MAX_MM: f64 = 1.5;
/// The angle at which a nib reaches its longest (81°).
const NIB_LONGEST_AT_RAD: f64 = std::f64::consts::FRAC_PI_2 * 0.9;

/// The longest a nib of `profile` grows.
fn nib_max_mm(profile_max: Option<f64>) -> f64 {
    match profile_max.unwrap_or(OLD_NIB_MAX_MM) {
        max if max == OLD_NIB_MAX_MM => NIB_MAX_MM,
        max => max,
    }
}

/// How long a nib is: its least length while the cone's `axis` is within
/// the profile's angle of the surface's `normal`, growing evenly from there
/// to its longest at 81°. Never shorter than half the contact (the round
/// tip's ball sits on its top face); 0.1 mm without a profile.
pub fn nib_length(normal: Vec3, axis: Vec3, profile: Option<&ContactDiskProfile>) -> f64 {
    let Some(profile) = profile else {
        return NIB_MIN_MM;
    };
    let threshold = profile
        .standoff_angle_threshold
        .unwrap_or(NIB_GROWS_PAST_RAD);
    let least = profile
        .disk_thickness_mm
        .unwrap_or(NIB_MIN_MM)
        .max(profile.contact_diameter_mm.unwrap_or(0.0).max(0.0) / 2.0);
    let longest = least.max(nib_max_mm(profile.max_standoff_mm));
    let (n2, a2) = (normal.length_sq(), axis.length_sq());
    if n2 < 1e-8 || a2 < 1e-8 {
        return least;
    }
    let cos =
        (normal.x * axis.x + normal.y * axis.y + normal.z * axis.z) * (1.0 / (n2 * a2).sqrt());
    let angle = cos.clamp(-1.0, 1.0).acos();
    if angle <= threshold {
        return least;
    }
    let grown = ((angle.min(NIB_LONGEST_AT_RAD) - threshold)
        / (NIB_LONGEST_AT_RAD - threshold).max(1e-8))
    .clamp(0.0, 1.0);
    least + grown * (longest - least)
}

/// The centre of a nib `thickness` long standing on `pos` along `normal`.
pub fn get_disk_center(pos: Vec3, normal: Vec3, thickness: f64) -> Vec3 {
    let half = thickness / 2.0;
    Vec3::new(
        pos.x + normal.x * half,
        pos.y + normal.y * half,
        pos.z + normal.z * half,
    )
}

/// The turn that stands a cylinder (on its Y axis) along `normal`.
pub fn get_disk_rotation(normal: Vec3) -> Quaternion {
    Quaternion::set_from_unit_vectors(Vec3::new(0.0, 1.0, 0.0), normal.normalize())
}

/// How close a tip's cone may come to the model.
const TIP_CLEARANCE_MM: f64 = 0.3;

/// A nib long enough that the cone behind it clears the model, up to its
/// longest; the angle's length when the model is not in the way or not
/// known.
fn nib_clear_of_model(
    cone: &ContactCone,
    normal: Vec3,
    axis: Vec3,
    socket: Vec3,
    model: Option<&dyn CollisionField>,
) -> f64 {
    let profile = &cone.profile;
    if !profile.is_disk() {
        return 0.0;
    }
    let by_angle = nib_length(normal, axis, Some(&profile.as_disk_profile()));
    let Some(model) = model else {
        return by_angle;
    };
    let longest = nib_max_mm(profile.max_standoff_mm);
    if longest <= by_angle + 1e-6 {
        return by_angle;
    }
    let body = profile.body_diameter_mm / 2.0;
    let clear = calculate_safe_offset(
        cone.pos,
        normal,
        socket,
        body + TIP_CLEARANCE_MM,
        model,
        by_angle,
        longest,
        0.2,
        Some(CollisionFrustumProfile {
            start_radius: profile.contact_diameter_mm / 2.0 + TIP_CLEARANCE_MM,
            end_radius: body + TIP_CLEARANCE_MM,
            segment_count: None,
        }),
    );
    // Reaching the longest means nothing short of it cleared: keep the angle's.
    if clear >= longest - 1e-6 {
        by_angle
    } else {
        clear
    }
}

/// `cone` moved to `contact` on a surface facing `normal`, its socket staying
/// where it was (or at `socket`): a nib that clears the model, and the cone
/// from it straight to the socket.
pub fn recompute_contact_cone_for_moved_disk(
    cone: &ContactCone,
    contact: Vec3,
    normal: Vec3,
    socket: Option<Vec3>,
    model: Option<&dyn CollisionField>,
) -> ContactCone {
    let socket = socket.unwrap_or_else(|| get_final_socket_position(cone));
    let unit = |v: Vec3| v.length_sq() >= 1e-6;
    let surface = if unit(normal) {
        normal
    } else {
        Vec3::new(0.0, 0.0, 1.0)
    }
    .normalize();
    // The nib's length for an axis from the contact, then the axis from the
    // nib's end.
    let first = [socket.sub(contact), cone.normal, surface]
        .into_iter()
        .find(|&v| unit(v))
        .unwrap_or(surface)
        .normalize();
    let nib = nib_clear_of_model(cone, surface, first, socket, model);
    let start = contact.add(surface.multiply_scalar(nib));
    let mut axis = socket.sub(start);
    if !unit(axis) {
        axis = first;
    }
    let mut moved = cone.clone();
    moved.pos = contact;
    moved.surface_normal = Some(normal);
    moved.normal = axis.normalize();
    moved.profile.length_mm = axis.length().max(0.05);
    moved.disk_length_override = Some(nib);
    moved
}

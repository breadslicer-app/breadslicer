//! A branch: a support that grows from a knot on another support up to a
//! model contact. Its shaft runs from the knot to the socket of the contact
//! cone, with a joint halfway, or two when the cone had to be turned aside.

use support_geom::geom::math_utils::{
    deg_to_rad, js_atan2, js_cos, js_hypot, js_max, js_min, js_sin,
};
use support_model::collision_avoidance::{CollisionField, is_collision_frustum_blocked};
use support_model::cone_axis_policy::{
    ConeAngleMode, ConeAxisPolicyInput, normalize_vector_or_fallback, resolve_cone_axis_policy,
};
use support_model::constants::get_joint_diameter;
use support_model::rendering::support_builder::SupportData;
use support_model::settings::types::SupportSettings;
use support_model::support_kinds::SupportTypeId;
use support_model::tips::{
    ContactCone, SupportTipProfile, TipPrimitive, get_final_socket_position, nib_length,
    recompute_contact_cone_for_moved_disk,
};
use support_model::types::{Branch, Joint, Knot, Segment, SupportEntity, Vec3};

/// Air kept around a cone when testing it against the model, in mm.
const CONE_CLEARANCE_MM: f64 = 0.3;
/// How far a cone may tilt off the surface normal while the search looks for
/// room, in degrees. A branch's cone stays near its surface, as a trunk's does.
const TILTS_DEG: [f64; 4] = [0.0, 10.0, 20.0, 30.0];
/// Headings tried around the normal, nearest the cone's own heading first.
const HEADINGS_DEG: [f64; 12] = [
    0.0, 25.0, -25.0, 50.0, -50.0, 85.0, -85.0, 120.0, -120.0, 155.0, -155.0, 180.0,
];
/// Lengths tried, as multiples of the stock cone. Long cones look wrong on a
/// branch, so the stretch stays small.
const STRETCHES: [f64; 4] = [1.0, 1.05, 1.12, 1.2];
/// The flattest a cone may lie, in degrees off plumb, and still be preferred.
///
/// Flatter than this the socket has swung so far to the side that the cone
/// runs along the surface, a whisker that holds nothing up. Such a cone is
/// still taken when nothing steeper clears the model.
const MAX_PREFERRED_LEAN_DEG: f64 = 75.0;

#[derive(Clone, Copy, Debug)]
pub struct BranchBuildInput<'a> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    pub parent_knot: &'a Knot,
    pub mesh: Option<&'a dyn CollisionField>,
    /// Sizes of the auto-support tier. An auto branch is sized by its tier,
    /// never by the active preset; manual placement leaves these unset.
    pub shaft_diameter_mm: Option<f64>,
    pub tip_contact_diameter_mm: Option<f64>,
    pub roots_diameter_mm: Option<f64>,
    /// The settings to size from. A new branch leaves this unset and takes
    /// the live settings; rebuilding a branch passes its own, so an edit does
    /// not resize it to the active preset.
    pub settings: Option<&'a SupportSettings>,
    /// A tip to reuse as it is instead of one made from the settings. Its
    /// length is where the cone search starts; the returned cone carries the
    /// length found.
    pub tip_profile: Option<SupportTipProfile>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchBuildResult {
    pub branch: Branch,
    pub support_data: SupportData,
}

/// The contact the cone search works from.
#[derive(Clone, Copy)]
struct Contact {
    pos: Vec3,
    normal: Vec3,
    axis: Vec3,
    profile: SupportTipProfile,
}

impl Contact {
    fn draft(&self) -> ContactCone {
        ContactCone {
            id: "branch-draft".to_owned(),
            pos: self.pos,
            normal: self.axis,
            surface_normal: Some(self.normal),
            profile: self.profile,
            ..ContactCone::default()
        }
    }

    /// The cone on this contact whose socket lands on `socket`.
    fn cone_to(&self, socket: Vec3) -> ContactCone {
        recompute_contact_cone_for_moved_disk(
            &self.draft(),
            self.pos,
            self.normal,
            Some(socket),
            None,
        )
    }
}

/// How far the cone ending at `socket` leans off plumb, in degrees.
fn lean_deg(tip: Vec3, socket: Vec3) -> f64 {
    let across = js_hypot(&[socket.x - tip.x, socket.y - tip.y]);
    let up = (socket.z - tip.z).abs();
    (js_atan2(across, js_max(0.001, up)) * 180.0) / std::f64::consts::PI
}

/// Where the cone body starts, on top of its disk.
fn cone_base(cone: &ContactCone) -> Vec3 {
    let normal = cone.surface_normal.unwrap_or(cone.normal);
    let disk = cone.disk_length_override.unwrap_or_else(|| {
        if cone.profile.is_disk() {
            nib_length(normal, cone.normal, Some(&cone.profile.as_disk_profile()))
        } else {
            0.0
        }
    });
    Vec3::new(
        cone.pos.x + normal.x * disk,
        cone.pos.y + normal.y * disk,
        cone.pos.z + normal.z * disk,
    )
}

/// Whether the cone's body, with some air around it, stays out of the model.
fn cone_is_clear(cone: &ContactCone, mesh: Option<&dyn CollisionField>) -> bool {
    let Some(mesh) = mesh else {
        return true;
    };
    !is_collision_frustum_blocked(
        cone_base(cone),
        get_final_socket_position(cone),
        (cone.profile.contact_diameter_mm / 2.0) + CONE_CLEARANCE_MM,
        (cone.profile.body_diameter_mm / 2.0) + CONE_CLEARANCE_MM,
        mesh,
        None,
    )
}

/// What the search wants: the socket the stock cone would reach, the
/// direction toward it, and the stock length.
struct Goal {
    socket: Vec3,
    direction: Vec3,
    surface_normal: Vec3,
    length: f64,
}

impl Goal {
    /// The cost of a cone ending at `socket`; lower is better. Leaning off
    /// the surface normal costs most, then any length beyond stock, then
    /// distance from the wanted socket and from the wanted direction.
    fn cost(&self, socket: Vec3, cone: &ContactCone) -> f64 {
        let axis = normalize_vector_or_fallback(cone.normal, self.surface_normal);
        let unit_dot = |v: Vec3| js_max(-1.0, js_min(1.0, axis.dot(v)));
        let off_normal = (1.0 - unit_dot(self.surface_normal)) * 6.75;
        let off_direction = (1.0 - unit_dot(self.direction)) * 0.35;
        let off_socket = socket.distance_to(self.socket) * 0.95;
        let extra = js_max(0.0, cone.profile.length_mm - self.length);
        let stretch = extra * 5.5 + extra * extra * 1.6;
        off_normal + off_direction + off_socket + stretch
    }
}

#[derive(Clone)]
struct Choice {
    cone: ContactCone,
    socket: Vec3,
    cost: f64,
}

/// The cheapest option seen, and the cheapest of those steep enough to be
/// preferred.
#[derive(Default)]
struct Pick {
    any: Option<Choice>,
    steep: Option<Choice>,
}

impl Pick {
    fn offer(&mut self, tip: Vec3, option: Choice) {
        if self.any.as_ref().is_none_or(|b| option.cost < b.cost) {
            self.any = Some(option.clone());
        }
        if lean_deg(tip, option.socket) > MAX_PREFERRED_LEAN_DEG {
            return;
        }
        if self.steep.as_ref().is_none_or(|b| option.cost < b.cost) {
            self.steep = Some(option);
        }
    }

    /// A steep cone that clears wins over a closer one that does not: a
    /// cone lying along the surface holds nothing, and the shaft would bend
    /// into it at the socket.
    fn best(self) -> Option<Choice> {
        self.steep.or(self.any)
    }
}

/// The contact cone and its socket, and whether the cone was turned aside
/// from the stock one to clear the model.
struct PlacedCone {
    cone: ContactCone,
    socket: Vec3,
    turned: bool,
}

/// Find the cone for the contact: the stock cone when it clears, otherwise
/// the cheapest of a fan of tilted and stretched ones that does.
fn place_cone(contact: Contact, mesh: Option<&dyn CollisionField>) -> PlacedCone {
    let _perf = support_geom::perf::scope("branch:cone-search");
    let stock_socket = get_final_socket_position(&contact.draft());
    let stock = contact.cone_to(stock_socket);
    let stock_end = get_final_socket_position(&stock);

    let surface_normal = normalize_vector_or_fallback(contact.normal, Vec3::new(0.0, 0.0, 1.0));
    let disk_normal = stock.surface_normal.unwrap_or(contact.normal);
    let disk = stock.disk_length_override.unwrap_or(0.0);
    let base = Vec3::new(
        stock.pos.x + disk_normal.x * disk,
        stock.pos.y + disk_normal.y * disk,
        stock.pos.z + disk_normal.z * disk,
    );
    let goal = Goal {
        socket: stock_socket,
        direction: normalize_vector_or_fallback(stock_socket.sub(base), surface_normal),
        surface_normal,
        length: contact.profile.length_mm,
    };

    // A frame on the surface: "forward" is the wanted direction laid flat.
    let mut forward = goal
        .direction
        .sub(surface_normal.multiply_scalar(goal.direction.dot(surface_normal)));
    if forward.length_sq() < 0.000001 {
        forward = Vec3::new(1.0, 0.0, 0.0).cross(surface_normal);
        if forward.length_sq() < 0.000001 {
            forward = Vec3::new(0.0, 1.0, 0.0).cross(surface_normal);
        }
    }
    let forward = forward.normalize();
    let right = Vec3::cross_vectors(surface_normal, forward).normalize();

    let mut pick = Pick::default();
    if mesh.is_none() || cone_is_clear(&stock, mesh) {
        let cost = goal.cost(stock_end, &stock);
        pick.offer(
            contact.pos,
            Choice {
                cone: stock.clone(),
                socket: stock_end,
                cost,
            },
        );
    }

    for tilt in TILTS_DEG.map(deg_to_rad) {
        let (sin_tilt, cos_tilt) = (js_sin(tilt), js_cos(tilt));
        for heading in HEADINGS_DEG.map(deg_to_rad) {
            let sideways = forward
                .multiply_scalar(js_cos(heading) * sin_tilt)
                .add(right.multiply_scalar(js_sin(heading) * sin_tilt));
            let axis = surface_normal.multiply_scalar(cos_tilt).add(sideways);
            if axis.length_sq() < 0.000001 {
                continue;
            }
            let axis = axis.normalize();
            if axis.dot(surface_normal) < 0.12 {
                continue;
            }
            for stretch in STRETCHES {
                let length = js_max(goal.length, goal.length * stretch);
                let target = base.add(axis.multiply_scalar(length));
                let cone = contact.cone_to(target);
                if !cone_is_clear(&cone, mesh) {
                    continue;
                }
                let cost = goal.cost(target, &cone);
                let socket = get_final_socket_position(&cone);
                pick.offer(contact.pos, Choice { cone, socket, cost });
            }
        }
    }

    match pick.best() {
        Some(chosen) => {
            let moved = |a: f64, b: f64| (a - b).abs() > 0.0001;
            let turned = moved(chosen.socket.x, stock_end.x)
                || moved(chosen.socket.y, stock_end.y)
                || moved(chosen.socket.z, stock_end.z);
            PlacedCone {
                cone: chosen.cone,
                socket: chosen.socket,
                turned,
            }
        }
        None => PlacedCone {
            cone: stock,
            socket: stock_end,
            turned: false,
        },
    }
}

/// Build a branch from its knot to the contact: the contact cone, and a
/// shaft of two segments meeting at a joint halfway, or of three when the
/// cone was turned aside and the shaft has room to line up with it first.
///
/// `live_settings` are used when the input names no settings of its own.
pub fn build_branch_data(
    input: &BranchBuildInput<'_>,
    live_settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> BranchBuildResult {
    let _perf = support_geom::perf::scope("branch:build");
    let knot = input.parent_knot;
    let settings = input.settings.unwrap_or(live_settings);
    let tip = &settings.tip;

    let axis = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal: input.tip_normal,
        cone_angle_mode: tip.cone_angle_mode.unwrap_or(ConeAngleMode::Normal),
        adaptive_cone_angle_offset_deg: Some(tip.adaptive_cone_angle_offset_deg.unwrap_or(30.0)),
    })
    .cone_axis;
    let profile = input.tip_profile.unwrap_or(SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: input
            .tip_contact_diameter_mm
            .unwrap_or(tip.contact_diameter_mm),
        body_diameter_mm: tip.body_diameter_mm,
        length_mm: tip.length_mm,
        penetration_mm: tip.penetration_mm,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::FRAC_PI_4),
        break_point_mm: tip.breakpoint_mm.filter(|b| *b > 0.0),
    });
    let shaft_diameter = input
        .shaft_diameter_mm
        .unwrap_or(settings.shaft.diameter_mm);
    let joint_diameter = get_joint_diameter(shaft_diameter);

    let placed = place_cone(
        Contact {
            pos: input.tip_pos,
            normal: input.tip_normal,
            axis,
            profile,
        },
        input.mesh,
    );
    let socket = placed.socket;

    let mut joint_at = |pos: Vec3| Joint {
        id: new_id(),
        pos,
        diameter: joint_diameter,
    };
    let socket_joint = joint_at(socket);

    // When the cone was turned aside, an extra joint short of the socket
    // lets the last segment come in along the knot-to-socket line.
    let knot_pos = knot.pos;
    let span = socket.sub(knot_pos);
    let span_length = span.length();
    let mut inner_joints = Vec::new();
    if placed.turned && span_length > 1.8 {
        let direction = span.normalize();
        let backoff = js_min(
            js_max(span_length * 0.28, 0.9),
            js_max(span_length - 0.65, 0.9),
        );
        let approach = socket.sub(direction.multiply_scalar(backoff));
        if approach.distance_to(knot_pos) > 0.6 {
            inner_joints.push(joint_at(knot_pos.lerp(approach, 0.5)));
            inner_joints.push(joint_at(approach));
        }
    }
    if inner_joints.is_empty() {
        inner_joints.push(joint_at(Vec3::new(
            (knot_pos.x + socket.x) / 2.0,
            (knot_pos.y + socket.y) / 2.0,
            (knot_pos.z + socket.z) / 2.0,
        )));
    }

    // Segments bottom to top; the first starts at the knot, with no joint.
    let mut segments = Vec::new();
    let mut below: Option<&Joint> = None;
    for top in inner_joints.iter().chain([&socket_joint]) {
        let mut segment = Segment::straight(new_id(), shaft_diameter);
        segment.top_joint = Some(top.clone());
        segment.bottom_joint = below.cloned();
        segments.push(segment);
        below = Some(top);
    }

    let contact_cone = ContactCone {
        id: new_id(),
        socket_joint_id: Some(socket_joint.id.clone()),
        ..placed.cone
    };
    let id = new_id();
    let branch = Branch {
        base: SupportEntity {
            id: id.clone(),
            model_id: input.model_id.to_owned(),
            type_id: Some(SupportTypeId::Branch),
            ..SupportEntity::default()
        },
        parent_knot_id: knot.id.clone(),
        segments: segments.clone(),
        contact_cone: Some(contact_cone.clone()),
    };
    let support_data = SupportData {
        id,
        start_pos: Some(knot_pos),
        knot: Some(knot.clone()),
        segments,
        contact_cone: Some(contact_cone),
        ..SupportData::default()
    };

    BranchBuildResult {
        branch,
        support_data,
    }
}

//! DragonFruit `src/supports/SupportTypes/Branch/branchBuilder.ts`.
//!
//! Settings are passed in (DragonFruit's `input.settings ?? getSettings()`),
//! ids come from `new_id` (`uuidv4()`), and the mesh is its collision field.

use crate::supports::auto_support::parameter_sizing::{
    SizeOverrides, apply_sizing_overrides_to_settings,
};
use crate::supports::constants::get_joint_diameter;
use crate::supports::placement_logic::collision_avoidance::{
    CollisionField, is_collision_frustum_blocked,
};
use crate::supports::placement_logic::cone_axis_policy::{
    ConeAngleMode, ConeAxisPolicyInput, normalize_vector_or_fallback, resolve_cone_axis_policy,
};
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::support_settings_codec::encode_support_settings_hex;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::contact_cone::types::{
    ContactCone, SupportTipProfile, TipPrimitive,
};
use crate::supports::support_primitives::contact_disk::contact_disk_interaction::recompute_contact_cone_for_moved_disk;
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use crate::supports::support_type_registry::SupportTypeId;
use crate::supports::types::{Branch, Joint, Knot, Segment, SupportEntity, Vec3};
use crate::three::math_utils::{deg_to_rad, js_atan2, js_cos, js_hypot, js_max, js_min, js_sin};

const BRANCH_CONE_COLLISION_SAFETY_MM: f64 = 0.3;
/// Branch socket search: polar angles from surface normal. Capped at 30°
/// to match the trunk cone axis deviation limit — branches should not bend
/// the contact cone drastically away from the attachment surface.
const BRANCH_SOCKET_POLAR_DEG: [f64; 4] = [0.0, 10.0, 20.0, 30.0];
const BRANCH_SOCKET_AZIMUTH_DEG: [f64; 12] = [
    0.0, 25.0, -25.0, 50.0, -50.0, 85.0, -85.0, 120.0, -120.0, 155.0, -155.0, 180.0,
];
/// Stretch factors: how much to extend the cone beyond nominal length.
/// Kept conservative — long stretched cones look unnatural on branches.
const BRANCH_SOCKET_STRETCH_FACTORS: [f64; 4] = [1.0, 1.05, 1.12, 1.2];

/// How flat a cone may lie and still be preferred. Past this the contact reads as
/// a near-horizontal whisker: the socket has swung so far sideways of the tip
/// that the cone runs along the surface instead of away from it.
///
/// A preference, not a filter — a cone past it is still used when no steeper one
/// clears the mesh, which is what "if possible" means for a contact in a pocket.
const MAX_PREFERRED_CONE_LEAN_FROM_VERTICAL_DEG: f64 = 75.0;

/// The lean of the cone that ends at `socket_pos`, in degrees from vertical.
fn cone_lean_from_vertical_deg(tip_pos: Vec3, socket_pos: Vec3) -> f64 {
    let horizontal = js_hypot(&[socket_pos.x - tip_pos.x, socket_pos.y - tip_pos.y]);
    let vertical = (socket_pos.z - tip_pos.z).abs();
    (js_atan2(horizontal, js_max(0.001, vertical)) * 180.0) / std::f64::consts::PI
}

fn get_cone_start_position(cone: &ContactCone) -> Vec3 {
    let surface_normal = cone.surface_normal.unwrap_or(cone.normal);
    let thickness = cone.disk_length_override.unwrap_or_else(|| {
        if cone.profile.is_disk() {
            calculate_disk_thickness(
                surface_normal,
                cone.normal,
                Some(&cone.profile.as_disk_profile()),
            )
        } else {
            0.0
        }
    });

    Vec3::new(
        cone.pos.x + surface_normal.x * thickness,
        cone.pos.y + surface_normal.y * thickness,
        cone.pos.z + surface_normal.z * thickness,
    )
}

fn is_cone_placement_clear(cone: &ContactCone, mesh: Option<&dyn CollisionField>) -> bool {
    let Some(mesh) = mesh else { return true };

    let cone_start = get_cone_start_position(cone);
    let socket_pos = get_final_socket_position(cone);
    let contact_radius = (cone.profile.contact_diameter_mm / 2.0) + BRANCH_CONE_COLLISION_SAFETY_MM;
    let body_radius = (cone.profile.body_diameter_mm / 2.0) + BRANCH_CONE_COLLISION_SAFETY_MM;
    !is_collision_frustum_blocked(
        cone_start,
        socket_pos,
        contact_radius,
        body_radius,
        mesh,
        None,
    )
}

fn preview_cone(
    tip_pos: Vec3,
    tip_normal: Vec3,
    effective_cone_axis: Vec3,
    tip_profile: SupportTipProfile,
) -> ContactCone {
    ContactCone {
        id: "preview-branch-cone".to_owned(),
        pos: tip_pos,
        normal: effective_cone_axis,
        surface_normal: Some(tip_normal),
        profile: tip_profile,
        ..ContactCone::default()
    }
}

fn build_authored_branch_cone(
    tip_pos: Vec3,
    tip_normal: Vec3,
    effective_cone_axis: Vec3,
    tip_profile: SupportTipProfile,
    socket_target: Vec3,
) -> ContactCone {
    recompute_contact_cone_for_moved_disk(
        &preview_cone(tip_pos, tip_normal, effective_cone_axis, tip_profile),
        tip_pos,
        tip_normal,
        Some(socket_target),
        None,
    )
}

fn build_nominal_branch_socket_target(
    tip_pos: Vec3,
    tip_normal: Vec3,
    effective_cone_axis: Vec3,
    tip_profile: SupportTipProfile,
) -> Vec3 {
    get_final_socket_position(&preview_cone(
        tip_pos,
        tip_normal,
        effective_cone_axis,
        tip_profile,
    ))
}

fn score_branch_cone_candidate(
    socket_pos: Vec3,
    desired_socket: Vec3,
    cone: &ContactCone,
    surface_normal: Vec3,
    desired_direction: Vec3,
    nominal_length_mm: f64,
) -> f64 {
    let axis = normalize_vector_or_fallback(cone.normal, surface_normal);

    let surface_alignment_penalty =
        (1.0 - js_max(-1.0, js_min(1.0, axis.dot(surface_normal)))) * 6.75;
    let desired_direction_penalty =
        (1.0 - js_max(-1.0, js_min(1.0, axis.dot(desired_direction)))) * 0.35;
    let socket_distance_penalty = socket_pos.distance_to(desired_socket) * 0.95;
    let extra_length_mm = js_max(0.0, cone.profile.length_mm - nominal_length_mm);
    let extra_length_penalty = extra_length_mm * 5.5 + extra_length_mm * extra_length_mm * 1.6;

    surface_alignment_penalty
        + desired_direction_penalty
        + socket_distance_penalty
        + extra_length_penalty
}

struct ConePlacement {
    cone: ContactCone,
    socket_pos: Vec3,
    rerouted: bool,
}

#[derive(Clone)]
struct Candidate {
    cone: ContactCone,
    socket_pos: Vec3,
    score: f64,
}

fn find_best_branch_cone_placement(
    tip_pos: Vec3,
    tip_normal: Vec3,
    effective_cone_axis: Vec3,
    tip_profile: SupportTipProfile,
    mesh: Option<&dyn CollisionField>,
) -> ConePlacement {
    let _perf = crate::perf::scope("branch:cone-search");
    let nominal_socket_target =
        build_nominal_branch_socket_target(tip_pos, tip_normal, effective_cone_axis, tip_profile);
    let direct_cone = build_authored_branch_cone(
        tip_pos,
        tip_normal,
        effective_cone_axis,
        tip_profile,
        nominal_socket_target,
    );
    let direct_socket_pos = get_final_socket_position(&direct_cone);

    let surface_normal = normalize_vector_or_fallback(tip_normal, Vec3::new(0.0, 0.0, 1.0));
    let sn = direct_cone.surface_normal.unwrap_or(tip_normal);
    let override_len = direct_cone.disk_length_override.unwrap_or(0.0);
    let cone_start = Vec3::new(
        direct_cone.pos.x + sn.x * override_len,
        direct_cone.pos.y + sn.y * override_len,
        direct_cone.pos.z + sn.z * override_len,
    );
    let desired_socket = nominal_socket_target;
    let desired_vector = desired_socket.sub(cone_start);
    let _desired_distance = js_max(0.25, desired_vector.length());
    let desired_direction = normalize_vector_or_fallback(desired_vector, surface_normal);
    let nominal_length_mm = tip_profile.length_mm;

    let mut tangent_forward = desired_direction
        .sub(surface_normal.multiply_scalar(desired_direction.dot(surface_normal)));
    if tangent_forward.length_sq() < 0.000001 {
        tangent_forward = Vec3::new(1.0, 0.0, 0.0).cross(surface_normal);
        if tangent_forward.length_sq() < 0.000001 {
            tangent_forward = Vec3::new(0.0, 1.0, 0.0).cross(surface_normal);
        }
    }
    tangent_forward = tangent_forward.normalize();
    let tangent_right = Vec3::cross_vectors(surface_normal, tangent_forward).normalize();

    let mut best_candidate: Option<Candidate> = None;
    let mut best_preferred: Option<Candidate> = None;
    let mut consider = |candidate: Candidate| {
        if best_candidate
            .as_ref()
            .is_none_or(|b| candidate.score < b.score)
        {
            best_candidate = Some(candidate.clone());
        }
        if cone_lean_from_vertical_deg(tip_pos, candidate.socket_pos)
            > MAX_PREFERRED_CONE_LEAN_FROM_VERTICAL_DEG
        {
            return;
        }
        if best_preferred
            .as_ref()
            .is_none_or(|b| candidate.score < b.score)
        {
            best_preferred = Some(candidate);
        }
    };

    if mesh.is_none() || is_cone_placement_clear(&direct_cone, mesh) {
        let score = score_branch_cone_candidate(
            direct_socket_pos,
            desired_socket,
            &direct_cone,
            surface_normal,
            desired_direction,
            nominal_length_mm,
        );
        consider(Candidate {
            cone: direct_cone.clone(),
            socket_pos: direct_socket_pos,
            score,
        });
    }

    for polar_deg in BRANCH_SOCKET_POLAR_DEG {
        let polar_rad = deg_to_rad(polar_deg);
        let sin_polar = js_sin(polar_rad);
        let cos_polar = js_cos(polar_rad);

        for azimuth_deg in BRANCH_SOCKET_AZIMUTH_DEG {
            let azimuth_rad = deg_to_rad(azimuth_deg);
            let tangent_component = tangent_forward
                .multiply_scalar(js_cos(azimuth_rad) * sin_polar)
                .add(tangent_right.multiply_scalar(js_sin(azimuth_rad) * sin_polar));
            let axis = surface_normal
                .multiply_scalar(cos_polar)
                .add(tangent_component);
            if axis.length_sq() < 0.000001 {
                continue;
            }
            let axis = axis.normalize();

            if axis.dot(surface_normal) < 0.12 {
                continue;
            }

            for stretch in BRANCH_SOCKET_STRETCH_FACTORS {
                let candidate_length = js_max(nominal_length_mm, nominal_length_mm * stretch);
                let candidate_socket = cone_start.add(axis.multiply_scalar(candidate_length));
                let candidate_cone = build_authored_branch_cone(
                    tip_pos,
                    tip_normal,
                    effective_cone_axis,
                    tip_profile,
                    candidate_socket,
                );

                if !is_cone_placement_clear(&candidate_cone, mesh) {
                    continue;
                }

                let score = score_branch_cone_candidate(
                    candidate_socket,
                    desired_socket,
                    &candidate_cone,
                    surface_normal,
                    desired_direction,
                    nominal_length_mm,
                );

                let socket_pos = get_final_socket_position(&candidate_cone);
                consider(Candidate {
                    cone: candidate_cone,
                    socket_pos,
                    score,
                });
            }
        }
    }

    // The steepest cone that clears wins over the nearest one that does not:
    // a contact served by a cone lying along the surface holds nothing up, and
    // the member bends into it at the socket, which is the junction the preview
    // reads as a near-horizontal whisker.
    match best_preferred.or(best_candidate) {
        Some(chosen) => {
            let rerouted = (chosen.socket_pos.x - direct_socket_pos.x).abs() > 0.0001
                || (chosen.socket_pos.y - direct_socket_pos.y).abs() > 0.0001
                || (chosen.socket_pos.z - direct_socket_pos.z).abs() > 0.0001;
            ConePlacement {
                cone: chosen.cone,
                socket_pos: chosen.socket_pos,
                rerouted,
            }
        }
        None => ConePlacement {
            cone: direct_cone,
            socket_pos: direct_socket_pos,
            rerouted: false,
        },
    }
}

#[derive(Clone, Copy)]
pub struct BranchBuildInput<'a> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    pub parent_knot: &'a Knot,
    pub mesh: Option<&'a dyn CollisionField>,
    /// Auto-support tier overrides — when absent, the global settings band
    /// is used (manual placement). Auto-placed branches must size from the
    /// auto-support tier, not the active trunk preset.
    pub shaft_diameter_mm: Option<f64>,
    pub tip_contact_diameter_mm: Option<f64>,
    pub roots_diameter_mm: Option<f64>,
    /// Settings band to size from. Defaults to the live global settings, which is
    /// what a fresh placement wants. Rebuilding an existing branch passes that
    /// branch's own band instead, so editing it does not resize it to whatever
    /// the active preset happens to be.
    pub settings: Option<&'a SupportSettings>,
    /// Contact tip profile to reuse verbatim instead of deriving one from
    /// `settings`. Its `lengthMm` is the nominal the socket search starts from;
    /// the solved length replaces it in the returned cone.
    pub tip_profile: Option<SupportTipProfile>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchBuildResult {
    pub branch: Branch,
    pub support_data: SupportData,
}

/// Builds a branch with 2 segments and a middle joint (like trunks):
/// - Starts at the parent knot position
/// - Bottom segment: knot -> middle joint
/// - Top segment: middle joint -> socket joint
/// - Contact cone at the tip
///
/// `live_settings` is `getSettings()`, used when the input names no settings.
pub fn build_branch_data(
    input: &BranchBuildInput<'_>,
    live_settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> BranchBuildResult {
    let _perf = crate::perf::scope("branch:build");
    let BranchBuildInput {
        tip_pos,
        tip_normal,
        model_id,
        parent_knot,
        mesh,
        ..
    } = *input;

    let settings = input.settings.unwrap_or(live_settings);
    let settings_code_hex = encode_support_settings_hex(&apply_sizing_overrides_to_settings(
        settings,
        Some(&SizeOverrides {
            shaft_diameter_mm: input.shaft_diameter_mm,
            tip_contact_diameter_mm: input.tip_contact_diameter_mm,
            roots_diameter_mm: input.roots_diameter_mm,
            ..SizeOverrides::default()
        }),
    ));
    let cone_angle_mode = settings
        .tip
        .cone_angle_mode
        .unwrap_or(ConeAngleMode::Normal);
    let adaptive_cone_angle_offset_deg =
        settings.tip.adaptive_cone_angle_offset_deg.unwrap_or(30.0);

    let cone_axis = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal: tip_normal,
        cone_angle_mode,
        adaptive_cone_angle_offset_deg: Some(adaptive_cone_angle_offset_deg),
    })
    .cone_axis;

    let effective_cone_axis = cone_axis;
    let tip_profile = input.tip_profile.unwrap_or(SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: input
            .tip_contact_diameter_mm
            .unwrap_or(settings.tip.contact_diameter_mm),
        body_diameter_mm: settings.tip.body_diameter_mm,
        length_mm: settings.tip.length_mm,
        penetration_mm: settings.tip.penetration_mm,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::PI / 4.0),
        sphere_radius_ratio: None,
    });

    let shaft_diameter = input
        .shaft_diameter_mm
        .unwrap_or(settings.shaft.diameter_mm);
    let joint_diameter = get_joint_diameter(shaft_diameter);
    let ConePlacement {
        cone: authored_cone,
        socket_pos,
        rerouted,
    } = find_best_branch_cone_placement(
        tip_pos,
        tip_normal,
        effective_cone_axis,
        tip_profile,
        mesh,
    );

    let socket_joint = Joint {
        id: new_id(),
        pos: socket_pos,
        diameter: joint_diameter,
    };

    let knot_pos = parent_knot.pos;
    let socket_vec = socket_pos;
    let knot_vec = knot_pos;
    let span = socket_vec.sub(knot_vec);
    let span_length = span.length();
    let span_direction = if span_length > 0.000001 {
        span.normalize()
    } else {
        authored_cone.normal.normalize()
    };

    let mut segments: Vec<Segment> = Vec::new();

    let mut add_joint = |pos: Vec3| Joint {
        id: new_id(),
        pos,
        diameter: joint_diameter,
    };
    let segment = |id: String, top: &Joint, bottom: Option<&Joint>| {
        let mut seg = Segment::straight(id, shaft_diameter);
        seg.top_joint = Some(top.clone());
        seg.bottom_joint = bottom.cloned();
        seg
    };

    let mut planned: Option<(Joint, Joint)> = None;
    if rerouted && span_length > 1.8 {
        let approach_backoff = js_min(
            js_max(span_length * 0.28, 0.9),
            js_max(span_length - 0.65, 0.9),
        );
        let approach_vec = socket_vec.sub(span_direction.multiply_scalar(approach_backoff));
        let approach_distance_to_knot = approach_vec.distance_to(knot_vec);

        if approach_distance_to_knot > 0.6 {
            let middle_vec = knot_vec.lerp(approach_vec, 0.5);
            let middle_joint = add_joint(middle_vec);
            let approach_joint = add_joint(approach_vec);
            planned = Some((middle_joint, approach_joint));
        }
    }

    if let Some((middle_joint, approach_joint)) = planned {
        segments.push(segment(new_id(), &middle_joint, None));
        segments.push(segment(new_id(), &approach_joint, Some(&middle_joint)));
        segments.push(segment(new_id(), &socket_joint, Some(&approach_joint)));
    }

    if segments.is_empty() {
        let middle_joint_pos = Vec3::new(
            (knot_pos.x + socket_pos.x) / 2.0,
            (knot_pos.y + socket_pos.y) / 2.0,
            (knot_pos.z + socket_pos.z) / 2.0,
        );

        let middle_joint = Joint {
            id: new_id(),
            pos: middle_joint_pos,
            diameter: joint_diameter,
        };

        segments.push(segment(new_id(), &middle_joint, None));
        segments.push(segment(new_id(), &socket_joint, Some(&middle_joint)));
    }

    let contact_cone = ContactCone {
        id: new_id(),
        socket_joint_id: Some(socket_joint.id.clone()),
        ..authored_cone
    };

    let branch_id = new_id();
    let branch = Branch {
        base: SupportEntity {
            id: branch_id.clone(),
            model_id: model_id.to_owned(),
            settings_code_hex: Some(settings_code_hex),
            type_id: Some(SupportTypeId::Branch),
            ..SupportEntity::default()
        },
        parent_knot_id: parent_knot.id.clone(),
        segments: segments.clone(),
        contact_cone: Some(contact_cone.clone()),
    };

    let support_data = SupportData {
        id: branch_id,
        start_pos: Some(parent_knot.pos),
        knot: Some(parent_knot.clone()),
        segments,
        contact_cone: Some(contact_cone),
        ..SupportData::default()
    };

    BranchBuildResult {
        branch,
        support_data,
    }
}

/// Re-stamps rebuilt branch geometry with the ids the branch already carried.
///
/// A tip drag rebuilds the whole branch on every pointer move. Fresh ids there
/// change the contact cone's id, which drops its selection and unmounts the
/// drag HUD mid-drag, and would detach any knot hosted on the branch's own
/// shafts once the drag commits. Segments and joints are matched by position:
/// geometry the rebuild adds keeps its new ids.
pub fn remap_branch_geometry_ids(rebuilt: &Branch, previous: &Branch) -> Branch {
    let mut joint_id_map: Vec<(String, String)> = Vec::new();
    let map_set = |map: &mut Vec<(String, String)>, from: &str, to: &str| {
        if let Some(entry) = map.iter_mut().find(|(k, _)| k == from) {
            entry.1 = to.to_owned();
        } else {
            map.push((from.to_owned(), to.to_owned()));
        }
    };

    for (index, segment) in rebuilt.segments.iter().enumerate() {
        let previous_top_joint = previous
            .segments
            .get(index)
            .and_then(|s| s.top_joint.as_ref());
        if let (Some(top), Some(prev)) = (&segment.top_joint, previous_top_joint) {
            map_set(&mut joint_id_map, &top.id, &prev.id);
        }
    }

    let mapped = |id: &str| {
        joint_id_map
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.clone())
    };
    let remap_joint = |joint: &Option<Joint>| -> Option<Joint> {
        joint.as_ref().map(|j| match mapped(&j.id) {
            Some(id) if !id.is_empty() => Joint { id, ..j.clone() },
            _ => j.clone(),
        })
    };

    let segments: Vec<Segment> = rebuilt
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| Segment {
            id: previous
                .segments
                .get(index)
                .map_or_else(|| segment.id.clone(), |p| p.id.clone()),
            top_joint: remap_joint(&segment.top_joint),
            bottom_joint: remap_joint(&segment.bottom_joint),
            ..segment.clone()
        })
        .collect();

    let contact_cone = rebuilt.contact_cone.as_ref().map(|cone| ContactCone {
        id: previous
            .contact_cone
            .as_ref()
            .map_or_else(|| cone.id.clone(), |p| p.id.clone()),
        socket_joint_id: match &cone.socket_joint_id {
            Some(socket) if !socket.is_empty() => {
                Some(mapped(socket).unwrap_or_else(|| socket.clone()))
            }
            other => other.clone(),
        },
        ..cone.clone()
    });

    Branch {
        segments,
        contact_cone,
        ..rebuilt.clone()
    }
}

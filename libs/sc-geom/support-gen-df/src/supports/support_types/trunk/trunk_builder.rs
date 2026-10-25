//! DragonFruit `src/supports/SupportTypes/Trunk/trunkBuilder.ts`.
//!
//! Trunk Builder
//!
//! Defines what a trunk is made of and builds the data structure.
//! Used for both preview and placement (same function, no duplication).
//!
//! `build_trunk_data` takes the mesh-free path (`calculateStandardPlacement`);
//! with a mesh DragonFruit routes through `calculateSmartPlacementV3`, which is
//! [`build_trunk_data_routed`]; both feed [`build_trunk_data_from_placement`]. Settings are passed in (DragonFruit's
//! `getSettings()`), ids come from `new_id` (`uuidv4()`), and the mesh is its
//! collision field.

use super::trunk_construction_joints::{
    normalize_first_construction_joint, with_central_straight_support_joint,
};
use super::trunk_route_types::{
    SnappedRoute, SnappedRouteValidity, TrunkRoute, TrunkRouteKind, TrunkRouteResult,
    TrunkRouteValidity,
};
use crate::supports::auto_support::parameter_sizing::{
    SizeOverrides, apply_sizing_overrides_to_settings,
};
use crate::supports::constants::get_joint_diameter;
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::placement_logic::grid::grid_math::grid_snapped_xy_from_key;
use crate::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use crate::supports::placement_logic::standard_placement::{
    TrunkPlacementInput, TrunkPlacementResult, calculate_standard_placement,
};
use crate::supports::placement_logic_v3::smart_placement_v3::{
    RouterStats, SmartPlacementV3Context, SmartPlacementV3Input, calculate_smart_placement_v3,
};
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::support_settings_codec::encode_support_settings_hex;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::{
    get_contact_cone_lowest_z, get_final_socket_position, get_socket_position,
};
use crate::supports::support_primitives::contact_cone::types::{
    ContactCone, SupportTipProfile, TipPrimitive,
};
use crate::supports::support_primitives::contact_disk::contact_disk_interaction::recompute_contact_cone_for_moved_disk;
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use crate::supports::support_type_registry::SupportTypeId;
use crate::supports::types::{
    Joint, LimitationCode, Quaternion, Roots, Segment, SupportEntity, Transform, Trunk, Vec3,
    WarningCode,
};
use crate::three::Matrix4;
use crate::three::math_utils::{js_max, js_min};

const JOINT_CHAIN_Z_EPSILON: f64 = 0.0001;
const BUILD_PLATE_Z_EPSILON_MM: f64 = 1e-6;

fn get_ascending_joint_penalty(joints: &[Vec3], lower_bound_z: f64, upper_bound_z: f64) -> f64 {
    let mut penalty = 0.0;
    let mut previous_z = lower_bound_z;

    for joint in joints {
        if joint.z <= previous_z + JOINT_CHAIN_Z_EPSILON {
            penalty += (previous_z + JOINT_CHAIN_Z_EPSILON) - joint.z + 1.0;
        }
        if joint.z >= upper_bound_z - JOINT_CHAIN_Z_EPSILON {
            penalty += joint.z - (upper_bound_z - JOINT_CHAIN_Z_EPSILON) + 1.0;
        }
        previous_z = js_max(previous_z, joint.z);
    }

    penalty
}

fn orient_joints_base_to_socket(
    joints: &[Vec3],
    lower_bound_z: f64,
    upper_bound_z: f64,
) -> Vec<Vec3> {
    if joints.len() < 2 {
        return joints.to_vec();
    }

    let forward = joints.to_vec();
    let mut reversed = joints.to_vec();
    reversed.reverse();
    let forward_penalty = get_ascending_joint_penalty(&forward, lower_bound_z, upper_bound_z);
    let reversed_penalty = get_ascending_joint_penalty(&reversed, lower_bound_z, upper_bound_z);

    if reversed_penalty < forward_penalty {
        reversed
    } else {
        forward
    }
}

fn filter_strictly_ascending_joints(
    joints: &[Vec3],
    lower_bound_z: f64,
    upper_bound_z: f64,
) -> Vec<Vec3> {
    let mut result = Vec::new();
    let mut previous_z = lower_bound_z;

    for joint in joints {
        if joint.z <= previous_z + JOINT_CHAIN_Z_EPSILON {
            continue;
        }
        if joint.z >= upper_bound_z - JOINT_CHAIN_Z_EPSILON {
            continue;
        }

        result.push(*joint);
        previous_z = joint.z;
    }

    result
}

struct JointChains {
    route_joints: Vec<Vec3>,
    construction_joints: Vec<Vec3>,
}

fn normalize_trunk_joint_chain(
    root_top_z: f64,
    socket_pos: Vec3,
    route_joints: &[Vec3],
    construction_joints: &[Vec3],
) -> JointChains {
    let oriented_construction =
        orient_joints_base_to_socket(construction_joints, root_top_z, socket_pos.z);
    let normalized_construction =
        filter_strictly_ascending_joints(&oriented_construction, root_top_z, socket_pos.z);

    let route_lower_bound_z = normalized_construction.last().map_or(root_top_z, |j| j.z);
    let oriented_route =
        orient_joints_base_to_socket(route_joints, route_lower_bound_z, socket_pos.z);
    let normalized_route =
        filter_strictly_ascending_joints(&oriented_route, route_lower_bound_z, socket_pos.z);

    JointChains {
        construction_joints: normalized_construction,
        route_joints: normalized_route,
    }
}

/// `TrunkBuildInput.overrides`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrunkOverrides {
    pub roots_diameter_mm: Option<f64>,
    pub roots_disk_height_mm: Option<f64>,
    pub roots_cone_height_mm: Option<f64>,
    pub shaft_diameter_mm: Option<f64>,
    pub tip_contact_diameter_mm: Option<f64>,
    pub tip_body_diameter_mm: Option<f64>,
    pub tip_length_mm: Option<f64>,
    pub tip_disk_length_override_mm: Option<f64>,
}

impl TrunkOverrides {
    /// The object as `applySizingOverridesToSettings` reads it.
    fn as_size_overrides(&self) -> SizeOverrides {
        SizeOverrides {
            shaft_diameter_mm: self.shaft_diameter_mm,
            tip_contact_diameter_mm: self.tip_contact_diameter_mm,
            tip_body_diameter_mm: self.tip_body_diameter_mm,
            tip_length_mm: self.tip_length_mm,
            tip_penetration_mm: None,
            roots_diameter_mm: self.roots_diameter_mm,
            roots_disk_height_mm: self.roots_disk_height_mm,
            roots_cone_height_mm: self.roots_cone_height_mm,
        }
    }
}

fn build_tip_profile(
    settings: &SupportSettings,
    overrides: Option<&TrunkOverrides>,
) -> SupportTipProfile {
    SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: overrides
            .and_then(|o| o.tip_contact_diameter_mm)
            .unwrap_or(settings.tip.contact_diameter_mm),
        body_diameter_mm: overrides
            .and_then(|o| o.tip_body_diameter_mm)
            .unwrap_or(settings.tip.body_diameter_mm),
        length_mm: overrides
            .and_then(|o| o.tip_length_mm)
            .unwrap_or(settings.tip.length_mm),
        penetration_mm: settings.tip.penetration_mm,
        disk_thickness_mm: Some(settings.tip.disk_thickness_mm.unwrap_or(0.1)),
        max_standoff_mm: Some(settings.tip.max_standoff_mm.unwrap_or(1.5)),
        standoff_angle_threshold: Some(
            settings
                .tip
                .standoff_angle_threshold
                .unwrap_or(std::f64::consts::PI / 4.0),
        ),
        sphere_radius_ratio: None,
    }
}

#[derive(Clone, Copy, Default)]
pub struct TrunkBuildInput<'a> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    pub mesh: Option<&'a dyn CollisionField>,
    /// When true, uses a fast first-pass preview search and then parity-checks
    /// collision outcomes against click-time tolerances before surfacing errors.
    pub is_preview: Option<bool>,
    pub overrides: Option<TrunkOverrides>,
    /// Small islands (voxel/minima <5mm² or z<15) bypass SDF — true straight, no central joint
    pub is_small_island: Option<bool>,
    /// Build as if grid mode were off: the base is routed to wherever the model
    /// allows instead of snapping to a lattice node.
    pub ignore_grid: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrunkBuildResult {
    pub root: Roots,
    pub trunk: Trunk,
    /// For SupportBuilder (generic format)
    pub support_data: SupportData,
    pub route: TrunkRoute,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<WarningCode>,
    /// True if the pathfinder stagnated (trapped in a cavity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stagnated: Option<bool>,
    /// True if V2 exhausted its expansion budget without reaching the goal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exhausted_budget: Option<bool>,
}

/// Build trunk data from a tip position and normal, without a mesh
/// (`calculateStandardPlacement`). A trunk consists of:
/// - Roots (disk + cone + sphere at base)
/// - Route-driven shaft segments and joints
/// - ContactCone at the tip
///
/// Returns `None` when a mesh is given: DragonFruit then routes with
/// `calculateSmartPlacementV3`, which is [`build_trunk_data_routed`].
pub fn build_trunk_data(
    input: &TrunkBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> Option<TrunkBuildResult> {
    if input.mesh.is_some() {
        return None;
    }
    let placement = calculate_standard_placement(&placement_input(input, settings), settings);
    Some(build_trunk_data_from_placement(
        input, &placement, settings, new_id,
    ))
}

/// The router's half of `buildTrunkData` with a mesh: the model's pooled
/// distance field, the mesh's world matrix, and the run's router counters.
pub struct TrunkRouter<'a> {
    pub sdf: &'a mut SdfCache,
    pub matrix_world: Matrix4,
    pub stats: &'a mut RouterStats,
}

/// Build trunk data with the model mesh: the V3 router (SDF-backed, bounded)
/// places it, with the same collision checks for preview and click, and the
/// placement is built against that same field.
pub fn build_trunk_data_routed(
    input: &TrunkBuildInput<'_>,
    router: &mut TrunkRouter<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> TrunkBuildResult {
    let placement = calculate_smart_placement_v3(
        &SmartPlacementV3Input {
            placement: placement_input(input, settings),
            model_id: input.model_id,
            is_preview: input.is_preview,
        },
        router.sdf,
        router.matrix_world,
        router.stats,
        settings,
        SmartPlacementV3Context {
            ignore_grid: input.ignore_grid == Some(true),
        },
    );
    let field: &dyn CollisionField = &*router.sdf;
    let routed = TrunkBuildInput {
        mesh: Some(field),
        ..*input
    };
    build_trunk_data_from_placement(&routed, &placement, settings, new_id)
}

/// What both placements are asked: the tip, its profile and the roots' top.
fn placement_input(input: &TrunkBuildInput<'_>, settings: &SupportSettings) -> TrunkPlacementInput {
    let overrides = input.overrides.as_ref();
    let tip_profile = build_tip_profile(settings, overrides);
    let disk_height = overrides
        .and_then(|o| o.roots_disk_height_mm)
        .unwrap_or(settings.roots.disk_height_mm);
    let cone_height = overrides
        .and_then(|o| o.roots_cone_height_mm)
        .unwrap_or(settings.roots.cone_height_mm);

    TrunkPlacementInput {
        tip_pos: input.tip_pos,
        tip_normal: input.tip_normal,
        tip_profile,
        // Roots dimensions: where the Roots sphere center is
        roots_top_z: disk_height + cone_height,
        is_small_island: input.is_small_island,
    }
}

pub fn build_trunk_data_from_placement(
    input: &TrunkBuildInput<'_>,
    placement: &TrunkPlacementResult,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> TrunkBuildResult {
    let TrunkBuildInput {
        tip_pos,
        tip_normal,
        model_id,
        mesh,
        ..
    } = *input;
    let overrides = input.overrides.as_ref();
    // Hex must describe the geometry actually built — tier overrides applied
    // (auto supports) — or Support Studio loads the global parameters instead
    // of the support's own.
    let settings_code_hex = encode_support_settings_hex(&apply_sizing_overrides_to_settings(
        settings,
        overrides.map(|o| o.as_size_overrides()).as_ref(),
    ));
    let tip_profile = build_tip_profile(settings, overrides);
    let tip_disk_length_override_mm = overrides.and_then(|o| o.tip_disk_length_override_mm);
    let shaft_diameter = overrides
        .and_then(|o| o.shaft_diameter_mm)
        .unwrap_or(settings.shaft.diameter_mm);
    let roots_diameter = overrides
        .and_then(|o| o.roots_diameter_mm)
        .unwrap_or(settings.roots.diameter_mm);
    let disk_height = overrides
        .and_then(|o| o.roots_disk_height_mm)
        .unwrap_or(settings.roots.disk_height_mm);
    let cone_height = overrides
        .and_then(|o| o.roots_cone_height_mm)
        .unwrap_or(settings.roots.cone_height_mm);
    let effective_cone_axis = placement.cone_axis.unwrap_or(tip_normal);
    let disk_thickness = if tip_profile.is_disk() {
        tip_disk_length_override_mm.unwrap_or_else(|| {
            calculate_disk_thickness(
                tip_normal,
                effective_cone_axis,
                Some(&tip_profile.as_disk_profile()),
            )
        })
    } else {
        0.0
    };

    let cone_start_pos = Vec3::new(
        tip_pos.x + tip_normal.x * disk_thickness,
        tip_pos.y + tip_normal.y * disk_thickness,
        tip_pos.z + tip_normal.z * disk_thickness,
    );

    let live_socket_pos =
        get_socket_position(cone_start_pos, effective_cone_axis, Some(&tip_profile));
    let solver_socket_pos = placement.socket_pos;
    let solver_socket_delta_sq = (solver_socket_pos.x - live_socket_pos.x)
        * (solver_socket_pos.x - live_socket_pos.x)
        + (solver_socket_pos.y - live_socket_pos.y) * (solver_socket_pos.y - live_socket_pos.y)
        + (solver_socket_pos.z - live_socket_pos.z) * (solver_socket_pos.z - live_socket_pos.z);

    // When the solver shifted the socket (cone-clear seed, A* routing), the
    // actual cone axis is the direction from coneStartPos to solverSocketPos —
    // NOT the surface-normal-derived axis.  Use the actual geometric direction
    // so the cone renderer orients the cone body to match the real geometry.
    let mut actual_cone_axis = effective_cone_axis;
    if solver_socket_delta_sq > 0.000001 {
        let delta_x = solver_socket_pos.x - cone_start_pos.x;
        let delta_y = solver_socket_pos.y - cone_start_pos.y;
        let delta_z = solver_socket_pos.z - cone_start_pos.z;
        let delta_len = (delta_x * delta_x + delta_y * delta_y + delta_z * delta_z).sqrt();
        if delta_len > 0.001 {
            actual_cone_axis = Vec3::new(
                delta_x / delta_len,
                delta_y / delta_len,
                delta_z / delta_len,
            );
        }
    }

    let contact_cone_template = recompute_contact_cone_for_moved_disk(
        &ContactCone {
            id: "__solver-authored-socket__".to_owned(),
            pos: tip_pos,
            normal: actual_cone_axis,
            surface_normal: Some(tip_normal),
            disk_length_override: tip_disk_length_override_mm,
            placement_surface: None,
            profile: tip_profile,
            socket_joint_id: None,
        },
        tip_pos,
        tip_normal,
        Some(solver_socket_pos),
        mesh,
    );
    let resolved_socket_pos = get_final_socket_position(&contact_cone_template);
    let joint_diameter = get_joint_diameter(shaft_diameter);
    let lowest_contact_z = js_min(
        get_contact_cone_lowest_z(&contact_cone_template),
        resolved_socket_pos.z - joint_diameter / 2.0,
    );
    let build_error = placement.error.or_else(|| {
        (lowest_contact_z < -BUILD_PLATE_Z_EPSILON_MM).then_some(LimitationCode::OutOfBounds)
    });
    let roots_top_z = disk_height + cone_height;
    let authored_route_joints = placement.joints.clone().unwrap_or_default();
    let authored_construction_joints = placement.construction_joints.clone().unwrap_or_default();
    let normalized_authored_chains = normalize_trunk_joint_chain(
        roots_top_z,
        resolved_socket_pos,
        &authored_route_joints,
        &authored_construction_joints,
    );
    let route_joints = normalized_authored_chains.route_joints;
    let is_straight_support = route_joints.is_empty();
    let small_island = input.is_small_island == Some(true);
    // Small islands: no central joint at all — true straight 1 segment even if grid offset 1mm is slanted slightly
    let initial_construction_joints = if small_island {
        Vec::new()
    } else {
        normalized_authored_chains.construction_joints
    };
    let fallback_construction_joints = if is_straight_support && !small_island {
        with_central_straight_support_joint(placement.base_pos, roots_top_z, resolved_socket_pos)
    } else {
        initial_construction_joints.clone()
    };
    let normalized_construction_joints = normalize_first_construction_joint(
        placement.base_pos,
        roots_top_z,
        resolved_socket_pos,
        &route_joints,
        if initial_construction_joints.is_empty() {
            &fallback_construction_joints
        } else {
            &initial_construction_joints
        },
    );
    let normalized_joint_chains = normalize_trunk_joint_chain(
        roots_top_z,
        resolved_socket_pos,
        &route_joints,
        &normalized_construction_joints,
    );
    let final_route_joints = normalized_joint_chains.route_joints;
    let final_construction_joints = normalized_joint_chains.construction_joints;
    let final_is_straight_support = final_route_joints.is_empty();

    let route_base = TrunkRouteResult {
        kind: if final_is_straight_support {
            TrunkRouteKind::Straight
        } else {
            TrunkRouteKind::Routed
        },
        base_pos: placement.base_pos,
        socket_pos: resolved_socket_pos,
        unsnapped_bottom_pos: placement.unsnapped_bottom_pos.unwrap_or(placement.base_pos),
        joints: final_route_joints,
        construction_joints: final_construction_joints,
        validity: if build_error.is_some() {
            TrunkRouteValidity::HardInvalid
        } else {
            TrunkRouteValidity::Valid
        },
        error: build_error,
        warning: placement.warning,
        angle: placement.angle,
        cone_axis: Some(effective_cone_axis),
        grid_ignored: placement.grid_ignored,
    };
    let route = match placement
        .snapped_node_key
        .as_deref()
        .filter(|key| !key.is_empty())
    {
        Some(snapped_node_key) => {
            let unsnapped = route_base.unsnapped_bottom_pos;
            let same_xy =
                unsnapped.x == route_base.base_pos.x && unsnapped.y == route_base.base_pos.y;
            let snapped_root_pos = if settings.grid.enabled {
                let (x, y) = grid_snapped_xy_from_key(snapped_node_key, settings.grid.spacing_mm);
                Vec3::new(x, y, route_base.base_pos.z)
            } else {
                route_base.base_pos
            };
            let snapped_validity = if build_error.is_some() {
                SnappedRouteValidity::HardInvalid
            } else if same_xy {
                SnappedRouteValidity::Valid
            } else {
                SnappedRouteValidity::InvalidAssisted
            };
            let validity = if build_error.is_some() {
                TrunkRouteValidity::HardInvalid
            } else if same_xy {
                TrunkRouteValidity::Valid
            } else {
                TrunkRouteValidity::RouteInvalid
            };
            TrunkRoute {
                route: TrunkRouteResult {
                    validity,
                    ..route_base
                },
                snapped: Some(SnappedRoute {
                    snapped_root_pos,
                    snapped_node_key: Some(snapped_node_key.to_owned()),
                    snapped_validity,
                }),
            }
        }
        None => TrunkRoute {
            route: route_base,
            snapped: None,
        },
    };

    let TrunkRouteResult {
        base_pos,
        socket_pos,
        joints,
        construction_joints,
        error,
        warning,
        angle,
        ..
    } = &route.route;
    let joint_positions: Vec<Vec3> = construction_joints
        .iter()
        .chain(joints.iter())
        .copied()
        .collect();

    // Generate IDs
    let root_id = new_id();
    let trunk_id = new_id();
    let contact_cone_id = new_id();
    // NEW: Generate a dedicated ID for the socket joint to ensure it is distinct from the knee.
    let socket_joint_id = new_id();

    // Build Joints and Segments
    // We need N joints and N+1 segments.
    let created_joints: Vec<Joint> = joint_positions
        .iter()
        .map(|pos| Joint {
            id: new_id(),
            pos: *pos,
            diameter: joint_diameter,
        })
        .collect();
    let mut created_segments: Vec<Segment> = Vec::new();

    // NEW: Calculate Socket Joint Position and Create it
    let socket_joint = Joint {
        id: socket_joint_id.clone(),
        pos: *socket_pos,
        diameter: joint_diameter,
    };

    let segment = |id: String, bottom: Option<&Joint>, top: &Joint| {
        let mut seg = Segment::straight(id, shaft_diameter);
        seg.bottom_joint = bottom.cloned();
        seg.top_joint = Some(top.clone());
        seg
    };

    // 2. Create Segments
    if final_is_straight_support && created_joints.len() == 1 {
        created_segments.push(segment(new_id(), None, &created_joints[0]));
        created_segments.push(segment(new_id(), Some(&created_joints[0]), &socket_joint));
    } else {
        for (index, joint) in created_joints.iter().enumerate() {
            let bottom = if index > 0 {
                Some(&created_joints[index - 1])
            } else {
                None
            };
            created_segments.push(segment(new_id(), bottom, joint));
        }
        created_segments.push(segment(new_id(), created_joints.last(), &socket_joint));
    }

    // Build Root
    let root = Roots {
        base: SupportEntity {
            id: root_id.clone(),
            model_id: model_id.to_owned(),
            ..SupportEntity::default()
        },
        transform: Transform {
            pos: *base_pos,
            rot: Quaternion::new(0.0, 0.0, 0.0, 1.0),
            scale: None,
        },
        diameter: roots_diameter,
        disk_height,
        cone_height,
        height: None,
    };

    // Build ContactCone
    let contact_cone = ContactCone {
        id: contact_cone_id,
        socket_joint_id: Some(socket_joint_id),
        ..contact_cone_template
    };

    // Build Trunk
    let trunk = Trunk {
        base: SupportEntity {
            id: trunk_id.clone(),
            model_id: model_id.to_owned(),
            settings_code_hex: Some(settings_code_hex),
            type_id: Some(SupportTypeId::Trunk),
            ..SupportEntity::default()
        },
        root_id,
        base_diameter_mm: Some(shaft_diameter),
        segments: created_segments.clone(),
        contact_cone: Some(contact_cone.clone()),
    };

    // Build generic SupportData for SupportBuilder
    let support_data = SupportData {
        id: trunk_id,
        roots: Some(root.clone()),
        segments: created_segments,
        contact_cone: Some(contact_cone),
        error: *error,
        warning: *warning,
        angle: *angle, // Required for gradient color
        ..SupportData::default()
    };

    let (error, warning) = (*error, *warning);
    TrunkBuildResult {
        root,
        trunk,
        support_data,
        route,
        error,
        warning,
        stagnated: placement.stagnated,
        exhausted_budget: placement.exhausted_budget,
    }
}

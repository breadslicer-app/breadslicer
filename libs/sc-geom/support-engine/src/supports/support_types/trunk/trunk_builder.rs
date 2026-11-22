//! A trunk: a support from its own root on the plate up to a model contact.
//!
//! A placement (the straight one without a mesh, the router with one) says
//! where the base, the socket and any bends go; this turns it into the root,
//! the shaft's joints and segments, and the contact cone. Preview and
//! placement build the same way.

use super::trunk_construction_joints::normalize_first_construction_joint;
use super::trunk_route_types::{
    SnappedRoute, SnappedRouteValidity, TrunkRoute, TrunkRouteKind, TrunkRouteResult,
    TrunkRouteValidity,
};
use crate::geom::Matrix4;
use crate::geom::math_utils::{js_max, js_min};
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
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_kinds::SupportTypeId;
use crate::supports::tips::{
    ContactCone, SupportTipProfile, TipPrimitive, get_contact_cone_lowest_z,
    get_final_socket_position, get_socket_position, nib_length,
    recompute_contact_cone_for_moved_disk,
};
use crate::supports::types::{
    Joint, LimitationCode, Quaternion, Roots, Segment, SupportEntity, Transform, Trunk, Vec3,
    WarningCode,
};

/// How much higher each joint must be than the one below it, in mm.
const RISE_EPSILON_MM: f64 = 0.0001;
/// How far below the plate a part of the trunk may reach before it is out of
/// bounds, in mm.
const PLATE_EPSILON_MM: f64 = 1e-6;

/// Sizes to build with instead of the settings', as the auto-support tiers
/// give them.
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
    /// Carried with the tier's sizes and recorded with them; the tip profile
    /// itself takes its penetration from the settings.
    pub tip_penetration_mm: Option<f64>,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct TrunkBuildInput<'a> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    pub mesh: Option<&'a dyn CollisionField>,
    /// A preview: the router searches quickly first, and checks a collision
    /// against the tolerances of a real placement before reporting it.
    pub is_preview: Option<bool>,
    pub overrides: Option<TrunkOverrides>,
    /// A small island (by voxel area or height): the router's construction
    /// joints are dropped and the distance field is not consulted.
    pub is_small_island: Option<bool>,
    /// Build as if the grid were off: the base goes wherever the model
    /// allows instead of to a grid node.
    pub ignore_grid: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrunkBuildResult {
    pub root: Roots,
    pub trunk: Trunk,
    /// The same support in the generic form the preview draws.
    pub support_data: SupportData,
    pub route: TrunkRoute,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<WarningCode>,
    /// The router stalled, boxed in by a cavity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stagnated: Option<bool>,
    /// The router ran out of search budget before reaching the plate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exhausted_budget: Option<bool>,
}

/// The router's share of a trunk built against the model: the model's pooled
/// distance field, its world matrix, and the run's router counters.
#[derive(Debug)]
pub struct TrunkRouter<'a> {
    pub sdf: &'a mut SdfCache,
    pub matrix_world: Matrix4,
    pub stats: &'a mut RouterStats,
}

/// The sizes a trunk is built with: the overrides where given, the settings
/// elsewhere.
struct Sizes {
    tip: SupportTipProfile,
    tip_disk_override: Option<f64>,
    shaft_diameter: f64,
    root_diameter: f64,
    root_disk_height: f64,
    root_cone_height: f64,
}

impl Sizes {
    fn of(settings: &SupportSettings, overrides: Option<&TrunkOverrides>) -> Self {
        let pick = |get: fn(&TrunkOverrides) -> Option<f64>, fallback: f64| {
            overrides.and_then(get).unwrap_or(fallback)
        };
        let tip = &settings.tip;
        let roots = &settings.roots;
        Sizes {
            tip: SupportTipProfile {
                kind: Some(TipPrimitive::Disk),
                contact_diameter_mm: pick(|o| o.tip_contact_diameter_mm, tip.contact_diameter_mm),
                body_diameter_mm: pick(|o| o.tip_body_diameter_mm, tip.body_diameter_mm),
                length_mm: pick(|o| o.tip_length_mm, tip.length_mm),
                penetration_mm: tip.penetration_mm,
                disk_thickness_mm: Some(tip.disk_thickness_mm.unwrap_or(0.1)),
                max_standoff_mm: Some(tip.max_standoff_mm.unwrap_or(1.5)),
                standoff_angle_threshold: Some(
                    tip.standoff_angle_threshold
                        .unwrap_or(std::f64::consts::FRAC_PI_4),
                ),
                break_point_mm: tip.breakpoint_mm.filter(|b| *b > 0.0),
            },
            tip_disk_override: overrides.and_then(|o| o.tip_disk_length_override_mm),
            shaft_diameter: pick(|o| o.shaft_diameter_mm, settings.shaft.diameter_mm),
            root_diameter: pick(|o| o.roots_diameter_mm, roots.diameter_mm),
            root_disk_height: pick(|o| o.roots_disk_height_mm, roots.disk_height_mm),
            root_cone_height: pick(|o| o.roots_cone_height_mm, roots.cone_height_mm),
        }
    }

    /// Height of the root's top, where the shaft starts.
    fn root_top_z(&self) -> f64 {
        self.root_disk_height + self.root_cone_height
    }
}

/// What a placement is asked: the contact, its tip, and the root's top.
fn placement_input(input: &TrunkBuildInput<'_>, settings: &SupportSettings) -> TrunkPlacementInput {
    let sizes = Sizes::of(settings, input.overrides.as_ref());
    TrunkPlacementInput {
        tip_pos: input.tip_pos,
        tip_normal: input.tip_normal,
        tip_profile: sizes.tip,
        roots_top_z: sizes.root_top_z(),
        is_small_island: input.is_small_island,
    }
}

/// A trunk placed without the model: straight down from the contact.
/// `None` when the input carries a mesh; such a trunk is built with
/// [`build_trunk_data_routed`].
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

/// A trunk placed by the router around the model, built and checked against
/// the same distance field the router used, preview or not.
pub fn build_trunk_data_routed(
    input: &TrunkBuildInput<'_>,
    router: &mut TrunkRouter<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> TrunkBuildResult {
    let _perf = crate::perf::scope("trunk:build-routed");
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
            ..Default::default()
        },
    );
    let field: &dyn CollisionField = &*router.sdf;
    let with_field = TrunkBuildInput {
        mesh: Some(field),
        ..*input
    };
    build_trunk_data_from_placement(&with_field, &placement, settings, new_id)
}

/// How badly a joint chain fails to climb strictly between `floor_z` and
/// `ceiling_z`: each joint at or below the highest one before it, or at or
/// above the ceiling, costs its overshoot plus one.
fn climb_penalty(joints: &[Vec3], floor_z: f64, ceiling_z: f64) -> f64 {
    let mut penalty = 0.0;
    let mut highest = floor_z;
    for joint in joints {
        if joint.z <= highest + RISE_EPSILON_MM {
            penalty += (highest + RISE_EPSILON_MM) - joint.z + 1.0;
        }
        if joint.z >= ceiling_z - RISE_EPSILON_MM {
            penalty += joint.z - (ceiling_z - RISE_EPSILON_MM) + 1.0;
        }
        highest = js_max(highest, joint.z);
    }
    penalty
}

/// The joints ordered bottom to top: as given, or reversed when that climbs
/// better.
fn bottom_to_top(joints: &[Vec3], floor_z: f64, ceiling_z: f64) -> Vec<Vec3> {
    let mut ordered = joints.to_vec();
    if joints.len() >= 2 {
        let reversed: Vec<Vec3> = joints.iter().rev().copied().collect();
        if climb_penalty(&reversed, floor_z, ceiling_z) < climb_penalty(joints, floor_z, ceiling_z)
        {
            ordered = reversed;
        }
    }
    ordered
}

/// The joints that climb strictly, each above the last one kept and below
/// the ceiling.
fn strictly_climbing(joints: &[Vec3], floor_z: f64, ceiling_z: f64) -> Vec<Vec3> {
    let mut kept = Vec::new();
    let mut last_z = floor_z;
    for joint in joints {
        if joint.z <= last_z + RISE_EPSILON_MM || joint.z >= ceiling_z - RISE_EPSILON_MM {
            continue;
        }
        kept.push(*joint);
        last_z = joint.z;
    }
    kept
}

/// The construction joints, then the route bends above them, each ordered
/// bottom to top and cut to a strict climb from the root's top to the socket.
fn climbing_chains(
    root_top_z: f64,
    socket_z: f64,
    route: &[Vec3],
    construction: &[Vec3],
) -> (Vec<Vec3>, Vec<Vec3>) {
    let chain = |joints: &[Vec3], floor_z: f64| {
        strictly_climbing(&bottom_to_top(joints, floor_z, socket_z), floor_z, socket_z)
    };
    let construction = chain(construction, root_top_z);
    let route_floor = construction.last().map_or(root_top_z, |j| j.z);
    (chain(route, route_floor), construction)
}

/// The route's grid snap, when the placement snapped to a node.
fn snapped_route(
    route: TrunkRouteResult,
    node_key: Option<&str>,
    settings: &SupportSettings,
) -> TrunkRoute {
    let Some(key) = node_key.filter(|key| !key.is_empty()) else {
        return TrunkRoute {
            route,
            snapped: None,
        };
    };
    let unsnapped = route.unsnapped_bottom_pos;
    let on_node = unsnapped.x == route.base_pos.x && unsnapped.y == route.base_pos.y;
    let snapped_root_pos = if settings.grid.enabled {
        let (x, y) = grid_snapped_xy_from_key(key, settings.grid.spacing_mm);
        Vec3::new(x, y, route.base_pos.z)
    } else {
        route.base_pos
    };
    let failed = route.error.is_some();
    let (validity, snapped_validity) = if failed {
        (
            TrunkRouteValidity::HardInvalid,
            SnappedRouteValidity::HardInvalid,
        )
    } else if on_node {
        (TrunkRouteValidity::Valid, SnappedRouteValidity::Valid)
    } else {
        (
            TrunkRouteValidity::RouteInvalid,
            SnappedRouteValidity::InvalidAssisted,
        )
    };
    TrunkRoute {
        route: TrunkRouteResult { validity, ..route },
        snapped: Some(SnappedRoute {
            snapped_root_pos,
            snapped_node_key: Some(key.to_owned()),
            snapped_validity,
        }),
    }
}

/// Build the trunk a placement describes.
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
    // The sizes recorded are the ones built with, overrides included, so an
    // auto support reopens with its own sizes rather than the global ones.
    let sizes = Sizes::of(settings, input.overrides.as_ref());
    let tip = sizes.tip;
    let root_top_z = sizes.root_top_z();

    // Where the stock cone would put the socket.
    let stock_axis = placement.cone_axis.unwrap_or(tip_normal);
    let disk = if tip.is_disk() {
        sizes
            .tip_disk_override
            .unwrap_or_else(|| nib_length(tip_normal, stock_axis, Some(&tip.as_disk_profile())))
    } else {
        0.0
    };
    let cone_start = Vec3::new(
        tip_pos.x + tip_normal.x * disk,
        tip_pos.y + tip_normal.y * disk,
        tip_pos.z + tip_normal.z * disk,
    );
    let stock_socket = get_socket_position(cone_start, stock_axis, Some(&tip));

    // When the placement moved the socket, the cone points from its start to
    // where the socket really is, so it is drawn along the real geometry.
    let socket_target = placement.socket_pos;
    let (mx, my, mz) = (
        socket_target.x - stock_socket.x,
        socket_target.y - stock_socket.y,
        socket_target.z - stock_socket.z,
    );
    let mut cone_axis = stock_axis;
    if mx * mx + my * my + mz * mz > 0.000001 {
        let (dx, dy, dz) = (
            socket_target.x - cone_start.x,
            socket_target.y - cone_start.y,
            socket_target.z - cone_start.z,
        );
        let length = (dx * dx + dy * dy + dz * dz).sqrt();
        if length > 0.001 {
            cone_axis = Vec3::new(dx / length, dy / length, dz / length);
        }
    }
    let cone = recompute_contact_cone_for_moved_disk(
        &ContactCone {
            id: "trunk-draft".to_owned(),
            pos: tip_pos,
            normal: cone_axis,
            surface_normal: Some(tip_normal),
            disk_length_override: sizes.tip_disk_override,
            placement_surface: None,
            profile: tip,
            socket_joint_id: None,
        },
        tip_pos,
        tip_normal,
        Some(socket_target),
        mesh,
    );
    let socket = get_final_socket_position(&cone);
    let joint_diameter = get_joint_diameter(sizes.shaft_diameter);

    // Nothing of the cone or the socket joint may reach below the plate.
    let lowest_z = js_min(
        get_contact_cone_lowest_z(&cone),
        socket.z - joint_diameter / 2.0,
    );
    let error = placement
        .error
        .or_else(|| (lowest_z < -PLATE_EPSILON_MM).then_some(LimitationCode::OutOfBounds));

    // The shaft's joints: construction joints below the route bends, both
    // climbing strictly. A small island drops the placement's construction
    // joints; any trunk with no bends gets the mid-shaft joint instead.
    let (bends, construction) = climbing_chains(
        root_top_z,
        socket.z,
        placement.joints.as_deref().unwrap_or_default(),
        placement.construction_joints.as_deref().unwrap_or_default(),
    );
    let construction = if input.is_small_island == Some(true) {
        Vec::new()
    } else {
        construction
    };
    let construction = normalize_first_construction_joint(
        placement.base_pos,
        root_top_z,
        socket,
        &bends,
        &construction,
    );
    let (bends, construction) = climbing_chains(root_top_z, socket.z, &bends, &construction);

    let route = snapped_route(
        TrunkRouteResult {
            kind: if bends.is_empty() {
                TrunkRouteKind::Straight
            } else {
                TrunkRouteKind::Routed
            },
            base_pos: placement.base_pos,
            socket_pos: socket,
            unsnapped_bottom_pos: placement.unsnapped_bottom_pos.unwrap_or(placement.base_pos),
            joints: bends,
            construction_joints: construction,
            validity: if error.is_some() {
                TrunkRouteValidity::HardInvalid
            } else {
                TrunkRouteValidity::Valid
            },
            error,
            warning: placement.warning,
            angle: placement.angle,
            cone_axis: Some(stock_axis),
            grid_ignored: placement.grid_ignored,
        },
        placement.snapped_node_key.as_deref(),
        settings,
    );
    let r = &route.route;

    let root_id = new_id();
    let trunk_id = new_id();
    let cone_id = new_id();
    let socket_joint_id = new_id();
    let joints: Vec<Joint> = r
        .construction_joints
        .iter()
        .chain(&r.joints)
        .map(|pos| Joint {
            id: new_id(),
            pos: *pos,
            diameter: joint_diameter,
        })
        .collect();
    let socket_joint = Joint {
        id: socket_joint_id.clone(),
        pos: r.socket_pos,
        diameter: joint_diameter,
    };
    // One segment up to each joint, the first from the root, and a last one
    // up to the socket.
    let mut segments = Vec::with_capacity(joints.len() + 1);
    let mut below: Option<&Joint> = None;
    for top in joints.iter().chain([&socket_joint]) {
        let mut segment = Segment::straight(new_id(), sizes.shaft_diameter);
        segment.bottom_joint = below.cloned();
        segment.top_joint = Some(top.clone());
        segments.push(segment);
        below = Some(top);
    }

    let entity = |id: String, type_id: Option<SupportTypeId>| SupportEntity {
        id,
        model_id: model_id.to_owned(),
        type_id,
        ..SupportEntity::default()
    };
    let root = Roots {
        base: entity(root_id.clone(), None),
        transform: Transform {
            pos: r.base_pos,
            rot: Quaternion::IDENTITY,
            scale: None,
        },
        diameter: sizes.root_diameter,
        disk_height: sizes.root_disk_height,
        cone_height: sizes.root_cone_height,
        height: None,
        join_cone_mm: settings.roots.join_cone_mm.filter(|j| *j > 0.0),
    };
    let contact_cone = ContactCone {
        id: cone_id,
        socket_joint_id: Some(socket_joint_id),
        ..cone
    };
    let trunk = Trunk {
        base: entity(trunk_id.clone(), Some(SupportTypeId::Trunk)),
        root_id,
        base_diameter_mm: Some(sizes.shaft_diameter),
        segments: segments.clone(),
        contact_cone: Some(contact_cone.clone()),
    };
    let (error, warning, angle) = (r.error, r.warning, r.angle);
    let support_data = SupportData {
        id: trunk_id,
        roots: Some(root.clone()),
        segments,
        contact_cone: Some(contact_cone),
        error,
        warning,
        // The preview colours the trunk by its angle.
        angle,
        ..SupportData::default()
    };

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

//! DragonFruit `src/supports/PlacementLogic/Grid/gridPlacement.ts` (with its
//! `types.ts`): where a built trunk candidate lands on the support grid — on
//! its node, attached to a host already standing there or beside it, or
//! refused.
//!
//! Two settings are read, as DragonFruit reads them: `settings` is the
//! decision's own (`args.settings`), `live` the global `getSettings()` the
//! member builders and the branch collision probe read. Ids come from
//! `new_id`; the mesh is the model's collision field.

use indexmap::IndexMap;
use serde::Serialize;

use super::grid_math::{grid_node_key_from_xy, grid_snapped_xy_from_key, js_number_to_string};
use crate::supports::auto_support::constants::MAX_AUTO_LEAF_SPAN_MM;
use crate::supports::constants::member_shaft_diameter_mm;
use crate::supports::placement_logic::collision_avoidance::{CollisionField, is_shaft_blocked};
use crate::supports::placement_logic::smart_placement_search_utils::{
    SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG, SOCKET_ELBOW_MAX_ANGLE_FROM_VERTICAL_DEG,
    TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG, distance_3d,
    get_length_aware_max_angle_from_vertical_deg, member_departure_angle_from_vertical_deg,
};
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::knot::knot_utils::calculate_knot_position_on_segment_from_t;
use crate::supports::support_type_registry::{
    CONTACT_OVERRIDE_REGISTRATIONS, HostRef, PlacedSupport, PlacementPrimitive, SupportEntityValue,
    SupportPlacementMetric, SupportPlacementSettingPath, SupportTypeId, grid_host_types,
    placement_of_resolved, select_type_for_placement,
};
use crate::supports::support_types::branch::branch_builder::{BranchBuildInput, build_branch_data};
use crate::supports::support_types::leaf::leaf_builder::{LeafBuildInput, build_leaf_data};
use crate::supports::support_types::stump::stump_auto_placement::{
    ContactOverrideRequest, build_stump_contact_override,
};
use crate::supports::support_types::trunk::trunk_builder::TrunkBuildResult;
use crate::supports::support_types::trunk::trunk_route_resolution::{
    get_default_snapped_validity, get_resolved_snapped_node_key, get_resolved_snapped_root_pos,
    get_resolved_snapped_validity, has_resolved_snapped_root,
};
use crate::supports::support_types::trunk::trunk_route_types::{
    SnappedRoute, SnappedRouteValidity, TrunkRouteValidity,
};
use crate::supports::types::{Knot, LimitationCode, Roots, SupportState, Trunk, Vec3};
use crate::three::math_utils::{js_acos, js_hypot, js_max, js_min, rad_to_deg};

/// Matches `validateAndCullOrphans`' post-thickening trunk check (segment
/// diameter/2 + 0.15): a trunk that passes this gate must not die in the cull.
const MIN_TRUNK_CLEARANCE_MM: f64 = 0.15;

/// Node key reported when the router resolved the placement with the grid out.
const GRID_UNSNAPPED_NODE_KEY: &str = "unsnapped";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GridPlacementRejectReason {
    KnotAboveTip,
    StumpBelowRoot,
    NoHostSegment,
    ModelMismatch,
    NoValidAttachment,
    CollisionWithModel,
    OutOfBounds,
}

impl GridPlacementRejectReason {
    /// A type's own refusal, cast into this module's reject codes.
    fn from_limitation(code: LimitationCode) -> Self {
        match code {
            LimitationCode::KnotAboveTip => GridPlacementRejectReason::KnotAboveTip,
            LimitationCode::StumpBelowRoot => GridPlacementRejectReason::StumpBelowRoot,
            LimitationCode::OutOfBounds => GridPlacementRejectReason::OutOfBounds,
            LimitationCode::CollisionWithModel => GridPlacementRejectReason::CollisionWithModel,
            // Not a reject code of this module; DragonFruit casts it through.
            LimitationCode::AngleTooSteep | LimitationCode::TooCloseToExisting => {
                GridPlacementRejectReason::NoValidAttachment
            }
        }
    }
}

// A decision is built once and moved to its caller; boxing the placement buys
// nothing on that path.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GridPlacementDecision {
    /// A support is placed on this contact, in the registry's generic shape.
    #[serde(rename_all = "camelCase")]
    Place {
        /// The grid node it landed on. Empty when a type's own override built it.
        node_key: String,
        placed: PlacedSupport,
        #[serde(skip_serializing_if = "Option::is_none")]
        support_data: Option<SupportData>,
    },
    #[serde(rename_all = "camelCase")]
    Reject {
        node_key: String,
        reason: GridPlacementRejectReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        trunk_build: Option<Box<TrunkBuildResult>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        support_data: Option<SupportData>,
    },
}

pub struct DecideGridPlacementArgs<'a> {
    pub settings: &'a SupportSettings,
    pub snapshot: &'a SupportState,
    pub candidate: &'a TrunkBuildResult,
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    pub mesh: Option<&'a dyn CollisionField>,
    /// When true, skip expensive click-time-only checks (unused by the decision).
    pub is_preview: Option<bool>,
}

/// A host as the grid sees it: the entity, the type that declares it, and the
/// root it stands on.
#[derive(Clone, Copy)]
struct GridHostEntry<'a> {
    host_type_id: SupportTypeId,
    host_id: &'a str,
    entity: &'a Trunk,
    root: &'a Roots,
}

/// Every host that indexes each node: where a pillar stands first, then the
/// contacts trunks carry. A node keeps a list, not a single host.
fn build_host_index<'a>(
    trunks: &[GridHostEntry<'a>],
    spacing_mm: f64,
) -> IndexMap<String, Vec<GridHostEntry<'a>>> {
    let mut index: IndexMap<String, Vec<GridHostEntry<'a>>> = IndexMap::new();
    let mut add = |key: String, entry: GridHostEntry<'a>| {
        let hosts = index.entry(key).or_default();
        if hosts.iter().any(|h| h.host_id == entry.host_id) {
            return;
        }
        hosts.push(entry);
    };
    for entry in trunks {
        let p = entry.root.transform.pos;
        add(grid_node_key_from_xy(p.x, p.y, spacing_mm), *entry);
    }
    // A base routed off the grid stands wherever 45 degrees allowed, so the
    // contact occupies its cell too.
    for entry in trunks {
        if let Some(cone) = &entry.entity.contact_cone {
            add(
                grid_node_key_from_xy(cone.pos.x, cone.pos.y, spacing_mm),
                *entry,
            );
        }
    }
    index
}

/// The trunks standing on this node, nearest first, within half a step of it.
fn hosts_near_node<'a>(
    trunks: &[GridHostEntry<'a>],
    node_key: &str,
    spacing_mm: f64,
) -> Vec<GridHostEntry<'a>> {
    let (cx, cy) = grid_snapped_xy_from_key(node_key, spacing_mm);
    let mut near: Vec<(GridHostEntry<'a>, f64)> = Vec::new();
    for entry in trunks {
        let p = entry.root.transform.pos;
        let mut distance_mm = js_hypot(&[p.x - cx, p.y - cy]);
        if let Some(cone) = &entry.entity.contact_cone {
            distance_mm = js_min(distance_mm, js_hypot(&[cone.pos.x - cx, cone.pos.y - cy]));
        }
        if distance_mm > spacing_mm * 0.5 {
            continue;
        }
        near.push((*entry, distance_mm));
    }
    near.sort_by(|a, b| {
        (a.1 - b.1)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    near.into_iter().map(|(e, _)| e).collect()
}

/// What every attachment search shares.
struct AttachContext<'a, 'b> {
    tip_pos: Vec3,
    tip_normal: Vec3,
    model_id: &'a str,
    min_angle_deg: f64,
    settings: &'a SupportSettings,
    live: &'a SupportSettings,
    attach_step_mm: f64,
    mesh: Option<&'a dyn CollisionField>,
    new_id: &'b mut dyn FnMut() -> String,
}

/// The first host of these that can actually take this contact.
fn attach_to_host(
    hosts: &[GridHostEntry<'_>],
    node_key: &str,
    ctx: &mut AttachContext<'_, '_>,
    occupied_point: bool,
) -> Option<GridPlacementDecision> {
    for host in hosts {
        if host.entity.segments.is_empty() {
            continue;
        }
        if let Some(decision) = select_attachment_decision(node_key, host, ctx, occupied_point) {
            return Some(decision);
        }
    }
    None
}

/// `withResolvedSnappedRoute`: the candidate with its snapped route fields,
/// validity and error set (the error also clears the warnings).
fn with_resolved_snapped_route(
    candidate: &TrunkBuildResult,
    snapped_root_pos: Vec3,
    snapped_node_key: Option<String>,
    snapped_validity: SnappedRouteValidity,
    validity: Option<TrunkRouteValidity>,
    error: Option<LimitationCode>,
) -> TrunkBuildResult {
    let next_error = error.or(candidate.route.route.error);
    let next_validity = validity.unwrap_or(candidate.route.route.validity);
    let mut out = candidate.clone();
    out.route.snapped = Some(SnappedRoute {
        snapped_root_pos,
        snapped_node_key,
        snapped_validity,
    });
    out.route.route.validity = next_validity;
    out.route.route.error = next_error;
    out.error = next_error;
    if next_error.is_some() {
        out.warning = None;
        out.support_data.warning = None;
    }
    out.support_data.error = next_error;
    out
}

/// Move the candidate's root (and every joint but the socket's) to (x, y).
fn move_root_to_xy(candidate: &TrunkBuildResult, root_x: f64, root_y: f64) -> TrunkBuildResult {
    let dx = root_x - candidate.root.transform.pos.x;
    let dy = root_y - candidate.root.transform.pos.y;
    if dx == 0.0 && dy == 0.0 {
        return candidate.clone();
    }

    let socket_joint_id = candidate
        .trunk
        .contact_cone
        .as_ref()
        .and_then(|c| c.socket_joint_id.clone());
    let shift = |joint: &mut Option<crate::supports::types::Joint>| {
        if let Some(j) = joint
            && socket_joint_id.as_deref() != Some(j.id.as_str())
        {
            j.pos.x += dx;
            j.pos.y += dy;
        }
    };
    let mut out = candidate.clone();
    for seg in &mut out.trunk.segments {
        shift(&mut seg.top_joint);
        shift(&mut seg.bottom_joint);
    }
    out.root.transform.pos.x = root_x;
    out.root.transform.pos.y = root_y;
    out.route.route.base_pos.x = root_x;
    out.route.route.base_pos.y = root_y;
    for joint in out
        .route
        .route
        .joints
        .iter_mut()
        .chain(out.route.route.construction_joints.iter_mut())
    {
        joint.x += dx;
        joint.y += dy;
    }
    out.support_data.roots = Some(out.root.clone());
    out.support_data.segments = out.trunk.segments.clone();
    out
}

fn apply_grid_snap_to_node_key(
    candidate: &TrunkBuildResult,
    spacing_mm: f64,
    node_key: &str,
) -> TrunkBuildResult {
    let (x, y) = grid_snapped_xy_from_key(node_key, spacing_mm);
    move_root_to_xy(candidate, x, y)
}

/// A trunk segment's two ends under the given settings' root stack.
fn trunk_segment_endpoints(
    trunk: &Trunk,
    root: &Roots,
    segment_index: usize,
    settings: &SupportSettings,
) -> Option<(Vec3, Vec3)> {
    let disk_height = settings.roots.disk_height_mm;
    let flare_enabled = settings.base_flare.enabled;
    let effective_cone_height = if flare_enabled {
        settings.base_flare.height_mm
    } else {
        0.0
    };
    let base = root.transform.pos;
    let plate_top = Vec3::new(base.x, base.y, base.z + disk_height + effective_cone_height);

    let segment = trunk.segments.get(segment_index)?;
    let start = match &segment.bottom_joint {
        Some(j) => j.pos,
        None if segment_index == 0 => plate_top,
        None => trunk.segments[segment_index - 1]
            .top_joint
            .as_ref()
            .map_or(plate_top, |j| j.pos),
    };
    let end = match (&segment.top_joint, &trunk.contact_cone) {
        (Some(j), _) => j.pos,
        (None, Some(cone)) => get_final_socket_position(cone),
        (None, None) => Vec3::new(start.x, start.y, start.z + 10.0),
    };
    Some((start, end))
}

fn branch_collides_with_mesh(
    knot: &Knot,
    tip_pos: Vec3,
    tip_normal: Vec3,
    mesh: &dyn CollisionField,
    shaft_diameter_mm: f64,
    live: &SupportSettings,
) -> bool {
    let _perf = crate::perf::scope("grid:branch-collision");
    let radius = shaft_diameter_mm / 2.0 + 0.25;
    let nominal_cone_length_mm = live.tip.length_mm;
    let socket_approx = Vec3::new(
        tip_pos.x + tip_normal.x * nominal_cone_length_mm,
        tip_pos.y + tip_normal.y * nominal_cone_length_mm,
        tip_pos.z + tip_normal.z * nominal_cone_length_mm,
    );
    // The full knot→socket segment as a single shaft (SDF-based).
    is_shaft_blocked(knot.pos, socket_approx, radius, mesh)
}

fn host_diameter_mm_from_knot(knot: &Knot, settings: &SupportSettings) -> f64 {
    js_max(
        0.001,
        knot.diameter.unwrap_or(settings.shaft.diameter_mm + 0.1) - 0.1,
    )
}

fn hosted_by(host: &GridHostEntry<'_>) -> Option<HostRef> {
    Some(HostRef {
        type_id: host.host_type_id,
        id: host.host_id.to_owned(),
    })
}

fn knot_supplied(knot: &Knot) -> IndexMap<String, PlacementPrimitive> {
    IndexMap::from([(
        "parentKnotId".to_owned(),
        PlacementPrimitive::Knot(knot.clone()),
    )])
}

/// A leaf from the knot to the tip, when the span is short enough to be one.
fn try_build_auto_leaf_decision(
    node_key: &str,
    host: &GridHostEntry<'_>,
    knot: &Knot,
    ctx: &mut AttachContext<'_, '_>,
) -> Option<GridPlacementDecision> {
    let tip_pos = ctx.tip_pos;
    let dx = tip_pos.x - knot.pos.x;
    let dy = tip_pos.y - knot.pos.y;
    let dz = tip_pos.z - knot.pos.z;
    let span_sq = dx * dx + dy * dy + dz * dz;
    let span_mm = span_sq.sqrt();
    let epsilon_z = 0.0001;
    if knot.pos.z > tip_pos.z + epsilon_z {
        return None;
    }
    if span_mm > MAX_AUTO_LEAF_SPAN_MM {
        return None;
    }

    let angle_from_up_deg = if span_sq < 0.000001 {
        0.0
    } else {
        rad_to_deg(js_acos(js_min(1.0, js_max(-1.0, dz / span_mm))))
    };
    let max_angle_deg = ctx.settings.shaft.max_angle_deg.unwrap_or(80.0);
    if angle_from_up_deg > max_angle_deg {
        return None;
    }

    let host_diameter_mm = host_diameter_mm_from_knot(knot, ctx.settings);
    // DragonFruit builds this leaf without the mesh.
    let built = build_leaf_data(
        &LeafBuildInput {
            tip_pos,
            surface_normal: ctx.tip_normal,
            model_id: ctx.model_id,
            parent_knot: knot,
            host_diameter_mm,
            tip_contact_diameter_mm: None,
            mesh: None,
        },
        ctx.live,
        ctx.new_id,
    );
    let type_id = built
        .leaf
        .base
        .type_id
        .expect("a built member carries its typeId");
    Some(GridPlacementDecision::Place {
        node_key: node_key.to_owned(),
        placed: placement_of_resolved(
            type_id,
            SupportEntityValue::Leaf(built.leaf),
            knot_supplied(knot),
            hosted_by(host),
        ),
        support_data: Some(built.support_data),
    })
}

/// Picks where a candidate attaches on a host trunk and builds that member, or
/// None when the host cannot take it. The first passing knot top-down whose
/// built member holds 45 degrees wins; otherwise the steepest built departure.
fn select_attachment_decision(
    node_key: &str,
    host: &GridHostEntry<'_>,
    ctx: &mut AttachContext<'_, '_>,
    occupied_point: bool,
) -> Option<GridPlacementDecision> {
    let _perf = crate::perf::scope("grid:attach-search");
    let host_trunk = host.entity;
    let tip_pos = ctx.tip_pos;
    let shaft_diameter_mm = ctx.settings.shaft.diameter_mm;
    // A member may not leave its host shallower than the configured branch
    // angle, except where the length-aware slack says otherwise.
    let base_max_from_vertical_deg = 90.0 - ctx.min_angle_deg;
    let member_allowance = |segment_mm: f64| {
        get_length_aware_max_angle_from_vertical_deg(
            segment_mm,
            base_max_from_vertical_deg,
            Some(base_max_from_vertical_deg),
        )
    };

    let mut best: Option<GridPlacementDecision> = None;
    let mut best_departure_deg = f64::INFINITY;
    for seg_index in (0..host_trunk.segments.len()).rev() {
        let segment = &host_trunk.segments[seg_index];
        let Some((start, end)) =
            trunk_segment_endpoints(host_trunk, host.root, seg_index, ctx.settings)
        else {
            continue;
        };
        // If the segment's bottom is above the tip, no point on it is below it.
        if start.z >= tip_pos.z {
            continue;
        }

        let approx_len = js_max(
            0.001,
            ((end.x - start.x) * (end.x - start.x)
                + (end.y - start.y) * (end.y - start.y)
                + (end.z - start.z) * (end.z - start.z))
                .sqrt(),
        );
        let step = js_max(0.0005, ctx.attach_step_mm / approx_len);

        let mut t = 1.0;
        while t >= -1e-9 {
            // The base (t=0) is always sampled.
            let tc = js_max(0.0, t);
            t -= step;
            let pos = calculate_knot_position_on_segment_from_t(start, end, segment, tc);
            if pos.z >= tip_pos.z {
                continue;
            }

            // Cheap chord gate: the loosest allowance the departure gate can grant.
            let chord_max = if occupied_point {
                SOCKET_ELBOW_MAX_ANGLE_FROM_VERTICAL_DEG
            } else {
                SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG
            };
            if member_departure_angle_from_vertical_deg(pos, tip_pos) > chord_max {
                continue;
            }

            let knot = Knot {
                id: (ctx.new_id)(),
                parent_shaft_id: segment.id.clone(),
                t: Some(tc),
                pos,
                diameter: Some(segment.diameter + 0.1),
                ..Knot::default()
            };

            if let Some(mesh) = ctx.mesh
                && branch_collides_with_mesh(
                    &knot,
                    tip_pos,
                    ctx.tip_normal,
                    mesh,
                    shaft_diameter_mm,
                    ctx.live,
                )
            {
                continue;
            }

            // A short span becomes a leaf, and its chord IS the member.
            if let Some(leaf_decision) = try_build_auto_leaf_decision(node_key, host, &knot, ctx) {
                let departure_deg = member_departure_angle_from_vertical_deg(pos, tip_pos);
                if departure_deg > member_allowance(distance_3d(pos, tip_pos)) {
                    continue;
                }
                if departure_deg <= TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG {
                    return Some(leaf_decision);
                }
                if departure_deg < best_departure_deg {
                    best = Some(leaf_decision);
                    best_departure_deg = departure_deg;
                }
                continue;
            }

            // A longer span becomes a branch; gate the built shaft where it
            // leaves the host, not the chord.
            let band_shaft_mm = ctx.settings.shaft.diameter_mm;
            let host_diameter_mm = host_diameter_mm_from_knot(&knot, ctx.settings);
            let floored_shaft_mm = member_shaft_diameter_mm(
                band_shaft_mm,
                host_diameter_mm,
                ctx.settings.auto_support.member_host_shaft_ratio,
            );
            let build_at = |shaft_mm: f64, ctx: &mut AttachContext<'_, '_>| {
                build_branch_data(
                    &BranchBuildInput {
                        tip_pos,
                        tip_normal: ctx.tip_normal,
                        model_id: ctx.model_id,
                        parent_knot: &knot,
                        mesh: ctx.mesh,
                        shaft_diameter_mm: Some(shaft_mm),
                        tip_contact_diameter_mm: None,
                        roots_diameter_mm: None,
                        settings: None,
                        tip_profile: None,
                    },
                    ctx.live,
                    ctx.new_id,
                )
            };
            let mut built = build_at(floored_shaft_mm, ctx);
            // The floor is a fit rule, not a licence to lose a graft: when the
            // thicker member does not clear, fall back to the band.
            if floored_shaft_mm > band_shaft_mm
                && let Some(mesh) = ctx.mesh
                && branch_collides_with_mesh(
                    &knot,
                    tip_pos,
                    ctx.tip_normal,
                    mesh,
                    floored_shaft_mm,
                    ctx.live,
                )
            {
                built = build_at(band_shaft_mm, ctx);
            }
            let first_joint = built
                .branch
                .segments
                .first()
                .and_then(|s| s.top_joint.as_ref())
                .map(|j| j.pos);
            let departure_deg =
                member_departure_angle_from_vertical_deg(pos, first_joint.unwrap_or(tip_pos));
            if let Some(first) = first_joint
                && departure_deg > member_allowance(distance_3d(pos, first))
            {
                continue;
            }

            let type_id = built
                .branch
                .base
                .type_id
                .expect("a built member carries its typeId");
            let decision = GridPlacementDecision::Place {
                node_key: node_key.to_owned(),
                placed: placement_of_resolved(
                    type_id,
                    SupportEntityValue::Branch(built.branch),
                    knot_supplied(&knot),
                    hosted_by(host),
                ),
                support_data: Some(built.support_data),
            };
            if departure_deg <= TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG {
                return Some(decision);
            }
            if departure_deg < best_departure_deg {
                best = Some(decision);
                best_departure_deg = departure_deg;
            }
        }
    }

    best
}

/// `Number(part)` for a node key part.
fn key_part(part: Option<&str>) -> f64 {
    match part.map(str::trim) {
        None => f64::NAN,
        Some("") => 0.0,
        Some(p) => p.parse::<f64>().unwrap_or(f64::NAN),
    }
}

/// How many rings of nodes a tip looks through for a pillar to hang from.
const GRID_NEIGHBOR_RINGS: i64 = 3;

fn find_neighbor_attachment(
    node_key: &str,
    hosts_by_node: &IndexMap<String, Vec<GridHostEntry<'_>>>,
    ctx: &mut AttachContext<'_, '_>,
) -> Option<GridPlacementDecision> {
    let mut parts = node_key.split(',');
    let gx = key_part(parts.next());
    let gy = key_part(parts.next());
    if !ctx.settings.grid.pillars {
        // Cardinals, then diagonals.
        const OFFSETS: [(f64, f64); 8] = [
            (0.0, 1.0),
            (0.0, -1.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (1.0, 1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (-1.0, -1.0),
        ];
        for (dx, dy) in OFFSETS {
            let neighbor_key = format!(
                "{},{}",
                js_number_to_string(gx + dx),
                js_number_to_string(gy + dy)
            );
            let hosts = hosts_by_node
                .get(&neighbor_key)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if let Some(decision) = attach_to_host(hosts, &neighbor_key, ctx, false) {
                return Some(decision);
            }
        }
        return None;
    }
    // Ring by ring outwards, nearest node first: the model can stand between a
    // tip and its own node, and a node further on may see it (#464). Each ring
    // opens only while a 45 degree run from the tip could still reach it.
    let reach_rings = ((ctx.tip_pos.z / ctx.settings.grid.spacing_mm).ceil() as i64)
        .clamp(1, GRID_NEIGHBOR_RINGS);
    for ring in 1..=reach_rings {
        let mut offsets: Vec<(i64, i64)> = Vec::new();
        for ix in -ring..=ring {
            for iy in -ring..=ring {
                if ix.abs() == ring || iy.abs() == ring {
                    offsets.push((ix, iy));
                }
            }
        }
        offsets.sort_by_key(|(ix, iy)| ix * ix + iy * iy);
        for (dx, dy) in offsets {
            let neighbor_key = format!(
                "{},{}",
                js_number_to_string(gx + dx as f64),
                js_number_to_string(gy + dy as f64)
            );
            let hosts = hosts_by_node
                .get(&neighbor_key)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if let Some(decision) = attach_to_host(hosts, &neighbor_key, ctx, false) {
                return Some(decision);
            }
        }
    }
    None
}

fn trunk_collides_with_mesh(
    candidate: &TrunkBuildResult,
    settings: &SupportSettings,
    mesh: &dyn CollisionField,
) -> bool {
    let _perf = crate::perf::scope("grid:trunk-collision");
    let trunk = &candidate.trunk;
    // Gate at the trunk's own (tier-sized) shaft, not the global setting.
    let max_seg_diameter = trunk
        .segments
        .iter()
        .fold(settings.shaft.diameter_mm, |m, s| js_max(m, s.diameter));
    let collision_radius = max_seg_diameter / 2.0 + MIN_TRUNK_CLEARANCE_MM;
    (0..trunk.segments.len()).any(|i| {
        trunk_segment_endpoints(trunk, &candidate.root, i, settings)
            .is_some_and(|(start, end)| is_shaft_blocked(start, end, collision_radius, mesh))
    })
}

fn reject(
    node_key: &str,
    reason: GridPlacementRejectReason,
    trunk_build: TrunkBuildResult,
) -> GridPlacementDecision {
    GridPlacementDecision::Reject {
        node_key: node_key.to_owned(),
        reason,
        trunk_build: Some(Box::new(trunk_build)),
        support_data: None,
    }
}

fn place_trunk(
    node_key: &str,
    type_id: SupportTypeId,
    build: TrunkBuildResult,
) -> GridPlacementDecision {
    let supplied = IndexMap::from([("rootId".to_owned(), PlacementPrimitive::Roots(build.root))]);
    GridPlacementDecision::Place {
        node_key: node_key.to_owned(),
        placed: placement_of_resolved(
            type_id,
            SupportEntityValue::Trunk(build.trunk),
            supplied,
            None,
        ),
        support_data: Some(build.support_data),
    }
}

pub fn decide_grid_placement(
    args: &DecideGridPlacementArgs<'_>,
    live: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> GridPlacementDecision {
    let _perf = crate::perf::scope("grid:decide");
    let DecideGridPlacementArgs {
        settings,
        snapshot,
        candidate,
        tip_pos,
        tip_normal,
        model_id,
        mesh,
        ..
    } = *args;

    let spacing_mm = settings.grid.spacing_mm;

    // Every host of this model, over every declared host type with a root.
    let mut trunks: Vec<GridHostEntry<'_>> = Vec::new();
    for descriptor in grid_host_types() {
        if !descriptor.owns_root || descriptor.id != SupportTypeId::Trunk {
            continue;
        }
        for (host_id, entity) in &snapshot.collections.trunks {
            if entity.base.model_id != model_id {
                continue;
            }
            let Some(root) = snapshot.collections.roots.get(&entity.root_id) else {
                continue;
            };
            trunks.push(GridHostEntry {
                host_type_id: descriptor.id,
                host_id,
                entity,
                root,
            });
        }
    }
    let hosts_by_node = build_host_index(&trunks, spacing_mm);

    // The type this tip height calls for. One registering a build override
    // takes this branch; otherwise this is the default type.
    let read_setting = |path: SupportPlacementSettingPath| match path {
        SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm => {
            Some(live.mesh_to_mesh.stick_vs_twig_cutoff_mm)
        }
    };
    let claimed_type_id =
        select_type_for_placement(SupportPlacementMetric::TipHeight, tip_pos.z, &read_setting);
    if let Some(claimed) = claimed_type_id
        && CONTACT_OVERRIDE_REGISTRATIONS.contains(&claimed)
    {
        // The stump is the one type registering an override.
        let built = build_stump_contact_override(
            &ContactOverrideRequest {
                tip_pos,
                tip_normal,
                model_id,
                mesh,
            },
            live,
            new_id,
        );
        if let Some(refusal) = built.refusal {
            return GridPlacementDecision::Reject {
                node_key: String::new(),
                reason: GridPlacementRejectReason::from_limitation(refusal),
                trunk_build: None,
                support_data: Some(built.support_data),
            };
        }
        return GridPlacementDecision::Place {
            node_key: String::new(),
            placed: built.placed,
            support_data: Some(built.support_data),
        };
    }

    let Some(placed_type_id) = claimed_type_id else {
        panic!("No support type claims a tipHeight of {}", tip_pos.z);
    };

    // A contact cone or socket below the plate cannot be rescued by snapping.
    if candidate.error == Some(LimitationCode::OutOfBounds) {
        let node_key = if settings.grid.enabled {
            get_resolved_snapped_node_key(&candidate.route)
                .map(str::to_owned)
                .unwrap_or_else(|| preferred_node_key(candidate, spacing_mm, None))
        } else {
            "disabled".to_owned()
        };
        return reject(
            &node_key,
            GridPlacementRejectReason::OutOfBounds,
            candidate.clone(),
        );
    }

    if !settings.grid.enabled {
        let build = with_resolved_snapped_route(
            candidate,
            get_resolved_snapped_root_pos(&candidate.route, candidate.root.transform.pos),
            Some("disabled".to_owned()),
            get_resolved_snapped_validity(&candidate.route)
                .unwrap_or_else(|| get_default_snapped_validity(&candidate.route)),
            None,
            None,
        );
        return place_trunk("disabled", placed_type_id, build);
    }

    let min_angle_deg = settings.grid.min_branch_angle_deg;
    let attach_step_mm = settings.grid.attach_search_step_mm;
    let resolved_node_key = get_resolved_snapped_node_key(&candidate.route).map(str::to_owned);
    let reference = candidate.route.route.unsnapped_bottom_pos;
    let node_key = resolved_node_key.clone().unwrap_or_else(|| {
        preferred_node_key(candidate, spacing_mm, Some((reference.x, reference.y)))
    });
    let node_hosts: Vec<GridHostEntry<'_>> =
        hosts_by_node.get(&node_key).cloned().unwrap_or_default();
    let snapped_candidate = if has_resolved_snapped_root(&candidate.route)
        && Some(&node_key) == resolved_node_key.as_ref()
    {
        candidate.clone()
    } else {
        apply_grid_snap_to_node_key(candidate, spacing_mm, &node_key)
    };
    let snapped_root_pos = get_resolved_snapped_root_pos(
        &snapped_candidate.route,
        snapped_candidate.root.transform.pos,
    );

    let mut ctx = AttachContext {
        tip_pos,
        tip_normal,
        model_id,
        min_angle_deg,
        settings,
        live,
        attach_step_mm,
        mesh,
        new_id,
    };

    if node_hosts.is_empty() {
        // Free by key, but a trunk may still stand on it (a root between nodes
        // is keyed to a neighbour): then the merge is the answer.
        let near_hosts = hosts_near_node(&trunks, &node_key, spacing_mm);
        if !near_hosts.is_empty() {
            if let Some(attachment) = attach_to_host(&near_hosts, &node_key, &mut ctx, true) {
                return attachment;
            }
            if let Some(neighbor) = find_neighbor_attachment(&node_key, &hosts_by_node, &mut ctx) {
                return neighbor;
            }
            // Occupied point, and no member can leave the trunk standing there.
            let build = with_resolved_snapped_route(
                &snapped_candidate,
                snapped_root_pos,
                Some(node_key.clone()),
                get_default_snapped_validity(&snapped_candidate.route),
                None,
                Some(
                    snapped_candidate
                        .route
                        .route
                        .error
                        .unwrap_or(LimitationCode::TooCloseToExisting),
                ),
            );
            return reject(
                &node_key,
                GridPlacementRejectReason::NoValidAttachment,
                build,
            );
        }

        // Grid-mode trunk candidates are built without the flexible mesh
        // router, so preview and click share this collision gate.
        let collides =
            mesh.is_some_and(|m| trunk_collides_with_mesh(&snapped_candidate, settings, m));
        if !collides {
            // The router already resolved this placement with the grid out.
            if candidate.route.route.grid_ignored == Some(true) {
                return place_trunk(GRID_UNSNAPPED_NODE_KEY, placed_type_id, candidate.clone());
            }
            let build = with_resolved_snapped_route(
                &snapped_candidate,
                snapped_root_pos,
                Some(node_key.clone()),
                get_resolved_snapped_validity(&snapped_candidate.route)
                    .unwrap_or_else(|| get_default_snapped_validity(&snapped_candidate.route)),
                None,
                None,
            );
            return place_trunk(&node_key, placed_type_id, build);
        }

        if let Some(neighbor) = find_neighbor_attachment(&node_key, &hosts_by_node, &mut ctx) {
            return neighbor;
        }

        let build = with_resolved_snapped_route(
            &snapped_candidate,
            snapped_root_pos,
            Some(node_key.clone()),
            SnappedRouteValidity::HardInvalid,
            Some(TrunkRouteValidity::HardInvalid),
            Some(LimitationCode::CollisionWithModel),
        );
        return reject(
            &node_key,
            GridPlacementRejectReason::CollisionWithModel,
            build,
        );
    }

    // A host trunk already occupies this grid node: attach to it, never
    // replace it.
    if node_hosts.iter().all(|h| h.entity.segments.is_empty()) {
        let build = with_resolved_snapped_route(
            &snapped_candidate,
            snapped_root_pos,
            Some(node_key.clone()),
            SnappedRouteValidity::HardInvalid,
            Some(TrunkRouteValidity::HardInvalid),
            Some(LimitationCode::TooCloseToExisting),
        );
        return reject(&node_key, GridPlacementRejectReason::NoHostSegment, build);
    }

    if let Some(attachment) = attach_to_host(&node_hosts, &node_key, &mut ctx, true) {
        return attachment;
    }
    if let Some(neighbor) = find_neighbor_attachment(&node_key, &hosts_by_node, &mut ctx) {
        return neighbor;
    }

    let build = with_resolved_snapped_route(
        &snapped_candidate,
        snapped_root_pos,
        Some(node_key.clone()),
        get_default_snapped_validity(&snapped_candidate.route),
        None,
        Some(
            snapped_candidate
                .route
                .route
                .error
                .unwrap_or(LimitationCode::TooCloseToExisting),
        ),
    );
    reject(
        &node_key,
        GridPlacementRejectReason::NoValidAttachment,
        build,
    )
}

/// `getPreferredNodeKey`: the node nearest the reference (the root by default).
fn preferred_node_key(
    candidate: &TrunkBuildResult,
    spacing_mm: f64,
    reference: Option<(f64, f64)>,
) -> String {
    let root = candidate.root.transform.pos;
    let (x, y) = reference.unwrap_or((root.x, root.y));
    grid_node_key_from_xy(x, y, spacing_mm)
}

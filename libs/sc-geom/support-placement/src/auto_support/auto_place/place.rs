//! One contact placed, as a click by hand places it too: hung from a
//! support nearby when it can be, else on a trunk of its own (sized for its
//! zone), else bridged from the model below when the trunk cannot reach the
//! plate; and then through the lattice when it is on.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::fan::{FanArgs, FanHostOrder, FanLeafResult, add_fanned, fan_leaf_to_host};
use super::helpers::{
    FanShaftPoint, HostedBranchInput, MergeHost, PlanCtx, branch_collides_with_sdf,
    branch_departure_angle_deg, build_hosted_branch, collect_fan_shaft_points_near,
    collect_stick_shaft_points_near, contact_cone_collides, entity_cones, find_merge_host,
    free_knot_id, host_entity_of, host_segment_span, is_host_at_attachment_capacity,
    member_max_angle_from_vertical_deg, origin_by_source,
};
use crate::auto_support::parameter_sizing::size_parameters;
use crate::auto_support::support_draft::{
    draft_add_entity, draft_add_primitive, draft_commit_support,
};
use crate::auto_support::types::{CandidatePoint, CandidateSource};
use crate::placement_logic::grid::grid_placement::{
    DecideGridPlacementArgs, GridPlacementDecision, GridPlacementRejectReason,
    decide_grid_placement,
};
use crate::support_types::contact_bridge::BridgeEntity;
use crate::support_types::leaf::leaf_builder::{LeafBuildInput, build_leaf_data};
use crate::support_types::trunk::cavity_bridge::{CavityBridgeSizing, build_cavity_bridge};
use crate::support_types::trunk::trunk_builder::{
    TrunkBuildInput, TrunkBuildResult, TrunkOverrides, TrunkRouter, build_trunk_data,
    build_trunk_data_routed,
};
use crate::support_types::trunk::trunk_route_resolution::get_resolved_snapped_root_pos;
use bread_support_geom::geom::math_utils::{js_atan2, js_hypot, js_max, js_min, js_to_fixed};
use bread_support_model::auto_support::constants::{
    CAVITY_FAN_RADIUS_MM, CAVITY_STICK_FAN_RADIUS_MM, CAVITY_STICK_MAX_MEMBERS,
    GRID_HOST_FAN_RADIUS_MM, MAX_CAVITY_BRIDGE_MM, MAX_LEAF_SPAN_BEFORE_BRANCH_MM,
    MIN_LEAF_FAN_RADIUS_MM,
};
use bread_support_model::auto_support::settings::SizingBand;
use bread_support_model::collision_avoidance::CollisionField;
use bread_support_model::cone_axis_policy::is_side_wall_contact;
use bread_support_model::model_mesh::ModelMesh;
use bread_support_model::support_kinds::{
    CONTACT_OVERRIDE_REGISTRATIONS, EndpointEnd, PlacementPrimitive, SupportEntityValue,
    SupportOriginId, SupportPlacementMetric, SupportPlacementSettingPath, SupportTypeId,
    contact_endpoints_for, get_support_type_descriptor, select_type_for_placement,
};
use bread_support_model::types::{Knot, LimitationCode, SupportState, Vec3};

/// Why a contact got no support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// Its trunk could not be built, and why.
    Trunk(LimitationCode),
    /// Its trunk's cone would lie within 15° of flat on the surface.
    SideWallContact,
    GridRejectCollision,
    GridRejectNoAttachment,
    GridRejectOther,
    AlreadySupported,
    Exception,
}

impl RejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectReason::Trunk(code) => match code {
                LimitationCode::AngleTooSteep => "trunk_angle_too_steep",
                LimitationCode::KnotAboveTip => "trunk_knot_above_tip",
                LimitationCode::StumpBelowRoot => "trunk_stump_below_root",
                LimitationCode::CollisionWithModel => "trunk_collision_with_model",
                LimitationCode::TooCloseToExisting => "trunk_too_close_to_existing",
                LimitationCode::OutOfBounds => "trunk_out_of_bounds",
            },
            RejectReason::SideWallContact => "side_wall_contact",
            RejectReason::GridRejectCollision => "grid_reject_collision",
            RejectReason::GridRejectNoAttachment => "grid_reject_no_attachment",
            RejectReason::GridRejectOther => "grid_reject_other",
            RejectReason::AlreadySupported => "already_supported",
            RejectReason::Exception => "exception",
        }
    }
}

/// What a contact came to: the support placed (`kind`, `entity_id`), or why
/// none was.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaceOutcome {
    pub kind: Option<SupportTypeId>,
    pub rejected_reason: Option<RejectReason>,
    pub entity_id: Option<String>,
    /// Why no support nearby took a bridged contact.
    pub cavity_fan_refusal: Option<String>,
    /// Placed off the lattice, which could not serve it, and why (#463).
    pub grid_fallback: bool,
    pub grid_fallback_reason: Option<&'static str>,
}

impl PlaceOutcome {
    fn placed(kind: SupportTypeId, entity_id: String) -> Self {
        PlaceOutcome {
            kind: Some(kind),
            entity_id: Some(entity_id),
            ..PlaceOutcome::default()
        }
    }

    fn rejected(reason: RejectReason) -> Self {
        PlaceOutcome {
            rejected_reason: Some(reason),
            ..PlaceOutcome::default()
        }
    }
}

fn set_origin(entity: &mut SupportEntityValue, origin: SupportOriginId) {
    let base = match entity {
        SupportEntityValue::Trunk(e) => &mut e.base,
        SupportEntityValue::Branch(e) => &mut e.base,
        SupportEntityValue::Leaf(e) => &mut e.base,
        SupportEntityValue::Twig(e) => &mut e.base,
        SupportEntityValue::Stick(e) => &mut e.base,
        SupportEntityValue::Brace(e) => &mut e.base,
        SupportEntityValue::Stump(e) => &mut e.base,
        SupportEntityValue::Kickstand(e) => &mut e.base,
    };
    base.origin = Some(origin);
}

fn entity_id(entity: &SupportEntityValue) -> String {
    match entity {
        SupportEntityValue::Trunk(e) => e.base.id.clone(),
        SupportEntityValue::Branch(e) => e.base.id.clone(),
        SupportEntityValue::Leaf(e) => e.base.id.clone(),
        SupportEntityValue::Twig(e) => e.base.id.clone(),
        SupportEntityValue::Stick(e) => e.base.id.clone(),
        SupportEntityValue::Brace(e) => e.base.id.clone(),
        SupportEntityValue::Stump(e) => e.base.id.clone(),
        SupportEntityValue::Kickstand(e) => e.base.id.clone(),
    }
}

/// The origin a hung member takes from what made its contact.
fn origin_of(source: CandidateSource) -> SupportOriginId {
    match source {
        CandidateSource::Voxel => SupportOriginId::Voxel,
        CandidateSource::Minima => SupportOriginId::Minima,
        CandidateSource::Intersection => SupportOriginId::Intersection,
        CandidateSource::Overhang => SupportOriginId::Overhang,
        CandidateSource::Stabilization => SupportOriginId::Stabilization,
        CandidateSource::Reinforcement => SupportOriginId::Reinforcement,
    }
}

/// What a run shares between contacts.
#[derive(Debug)]
pub struct PlaceArgs<'a> {
    /// The supports raised for the lattice's own points.
    pub grid_host_ids: Option<&'a HashSet<String>>,
}

/// The trunk under a contact, sized for it (routed round the model when it is
/// known).
fn sized_trunk(
    candidate: &CandidatePoint,
    tip_pos: Vec3,
    tip_normal: Vec3,
    preview: bool,
    small_island: Option<bool>,
    ignore_grid: bool,
    ctx: &PlanCtx<'_>,
) -> TrunkBuildResult {
    let size = size_parameters(candidate, ctx.live(), ctx.bands);
    let input = TrunkBuildInput {
        tip_pos,
        tip_normal,
        model_id: &candidate.model_id,
        mesh: None,
        is_preview: Some(preview),
        overrides: Some(TrunkOverrides {
            roots_diameter_mm: size.roots_diameter_mm,
            roots_disk_height_mm: size.roots_disk_height_mm,
            roots_cone_height_mm: size.roots_cone_height_mm,
            shaft_diameter_mm: size.shaft_diameter_mm,
            tip_contact_diameter_mm: size.tip_contact_diameter_mm,
            tip_body_diameter_mm: size.tip_body_diameter_mm,
            tip_length_mm: size.tip_length_mm,
            tip_disk_length_override_mm: None,
            tip_penetration_mm: size.tip_penetration_mm,
        }),
        is_small_island: small_island,
        ignore_grid: Some(ignore_grid),
    };
    match &ctx.mesh {
        Some(mesh) => {
            let mut sdf = mesh.sdf.borrow_mut();
            let mut stats = ctx.stats.borrow_mut();
            build_trunk_data_routed(
                &input,
                &mut TrunkRouter {
                    sdf: &mut sdf,
                    matrix_world: mesh.matrix,
                    stats: &mut stats,
                },
                ctx.live(),
                &mut || ctx.id(),
            )
        }
        None => build_trunk_data(&input, ctx.live(), &mut || ctx.id()).expect("mesh-free build"),
    }
}

/// Where a contact's support touches: a lattice point keeps the exact surface
/// it was read from, anything else is looked up.
fn contact_of(candidate: &CandidatePoint, ctx: &PlanCtx<'_>) -> (Vec3, Vec3) {
    if candidate.grid_point == Some(true) && candidate.tip_normal.z < 0.0 {
        (candidate.tip_pos, candidate.tip_normal)
    } else {
        ctx.contact_at(candidate.tip_pos)
    }
}

/// How far, in grid steps, a tip no node sees looks for a support to hang
/// from.
const GRID_PARENT_REACH_NODES: f64 = 1.5;

/// How high `candidate` meets the pillar of its grid node: where the 45°
/// run down from its tip meets the node's vertical; none when no node serves
/// it (#464).
pub(super) fn grid_join_height(candidate: &CandidatePoint, ctx: &PlanCtx<'_>) -> Option<f64> {
    let (tip_pos, tip_normal) = contact_of(candidate, ctx);
    let built = sized_trunk(candidate, tip_pos, tip_normal, true, None, false, ctx);
    if built.error.is_some() {
        return None;
    }
    let root = get_resolved_snapped_root_pos(&built.route, built.root.transform.pos);
    Some(tip_pos.z - (tip_pos.x - root.x).hypot(tip_pos.y - root.y))
}

/// Places `candidate` in `state` (left as it was when it gets no support).
/// `ignore_grid` is the second try of a contact the lattice could not serve.
pub fn place_one_candidate(
    candidate: &CandidatePoint,
    state: &mut SupportState,
    args: &PlaceArgs<'_>,
    ctx: &PlanCtx<'_>,
    ignore_grid: bool,
) -> PlaceOutcome {
    let live = ctx.live();
    let auto = &live.auto_support;
    let grid_on = live.grid.enabled && !ignore_grid;
    let mesh = ctx.mesh.as_ref();
    let draft: &SupportState = state;
    let model_id = candidate.model_id.as_str();
    let (tip_pos, tip_normal) = contact_of(candidate, ctx);
    let max_lean = js_min(
        auto.leaf_fan_max_angle_deg,
        member_max_angle_from_vertical_deg(live),
    );
    let fan_radius = js_max(MIN_LEAF_FAN_RADIUS_MM, auto.leaf_fan_radius_mm);
    let no_hosts = HashSet::new();

    // The shaft points near the tip, gathered once.
    let mut nearby: Option<Vec<FanShaftPoint>> = None;
    let mut near_points = || -> Vec<FanShaftPoint> {
        nearby
            .get_or_insert_with(|| {
                let reach = fan_radius
                    .max(GRID_HOST_FAN_RADIUS_MM)
                    .max(CAVITY_FAN_RADIUS_MM);
                collect_fan_shaft_points_near(draft, tip_pos, reach)
            })
            .clone()
    };
    #[allow(clippy::too_many_arguments)]
    let hang = |points: &[FanShaftPoint],
                grid_hosts: &HashSet<String>,
                prefix: &str,
                reach: f64,
                grid_reach: f64,
                cap: f64,
                draft: &SupportState,
                origin: SupportOriginId,
                order: FanHostOrder,
                rescue: Option<f64>| {
        match fan_leaf_to_host(
            &FanArgs {
                target: tip_pos,
                model_id,
                shaft_points: points,
                grid_host_ids: grid_hosts,
                knot_id_prefix: &format!("{prefix}-{}", candidate.id),
                fan_radius_mm: reach,
                grid_host_fan_radius_mm: grid_reach,
                max_angle_deg: max_lean,
                max_attachments: cap,
                draft,
                origin: Some(origin),
                host_order: order,
                rescue_span_override_mm: rescue,
            },
            ctx,
        ) {
            hung @ FanLeafResult::Ok { .. } => Ok(hung),
            refused => Err(refused),
        }
    };

    if !grid_on {
        // A lattice point off the lattice hangs from a support raised for an
        // island, never from a lattice one.
        if candidate.grid_point == Some(true)
            && candidate.source == CandidateSource::Overhang
            && !candidate.id.starts_with("grid-")
            && let Some(grid_hosts) = args.grid_host_ids
        {
            let points: Vec<FanShaftPoint> = near_points()
                .into_iter()
                .filter(|p| !grid_hosts.contains(&p.host_id))
                .collect();
            if !points.is_empty()
                && let Ok(hung) = hang(
                    &points,
                    &no_hosts,
                    "auto-fan",
                    fan_radius,
                    GRID_HOST_FAN_RADIUS_MM,
                    auto.max_attachments_per_trunk,
                    draft,
                    SupportOriginId::Overhang,
                    FanHostOrder::Steepest,
                    None,
                )
            {
                return hung_here(state, hung);
            }
        }
        // Any other contact (stabilisation anchors stand alone) hangs from a
        // support nearby, or joins the nearest trunk.
        if candidate.grid_point != Some(true) && candidate.source != CandidateSource::Stabilization
        {
            if candidate.source == CandidateSource::Overhang
                && let Some(grid_hosts) = args.grid_host_ids
                && let Ok(hung) = hang(
                    &near_points(),
                    grid_hosts,
                    "auto-fan",
                    fan_radius,
                    GRID_HOST_FAN_RADIUS_MM,
                    auto.max_attachments_per_trunk,
                    draft,
                    SupportOriginId::Overhang,
                    FanHostOrder::Steepest,
                    None,
                )
            {
                return hung_here(state, hung);
            }
            // An island's contact never joins a lattice trunk.
            let host = find_merge_host(tip_pos, model_id, draft, ctx.live().auto_support.share_hosts)
                .filter(|h| !args.grid_host_ids.is_some_and(|g| g.contains(&h.host_id)));
            if let Some(host) = host
                && let Some(joined) = join_trunk(candidate, tip_pos, tip_normal, &host, draft, ctx)
            {
                draft_add_primitive(state, PlacementPrimitive::Knot(joined.parent_knot));
                draft_add_entity(state, joined.kind, joined.member);
                return PlaceOutcome::placed(joined.kind, joined.id);
            }
        }
    }

    let small_island = (candidate.source != CandidateSource::Overhang
        && candidate.island_area_mm2 < 5.0)
        || candidate.z_height < 15.0;
    let trunk = sized_trunk(
        candidate,
        tip_pos,
        tip_normal,
        false,
        Some(small_island),
        ignore_grid,
        ctx,
    );

    // A tip low enough for a type of its own (built and checked its own way)
    // is not this trunk's to refuse.
    let setting = |path: SupportPlacementSettingPath| match path {
        SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm => {
            Some(live.mesh_to_mesh.stick_vs_twig_cutoff_mm)
        }
    };
    let own_type = (trunk.error == Some(LimitationCode::OutOfBounds))
        .then(|| select_type_for_placement(SupportPlacementMetric::TipHeight, tip_pos.z, &setting))
        .flatten()
        .is_some_and(|t| CONTACT_OVERRIDE_REGISTRATIONS.contains(&t));
    if let Some(error) = trunk.error
        && !own_type
    {
        if error == LimitationCode::CollisionWithModel
            && let Some(mesh) = mesh
        {
            let points = near_points();
            return bridged(
                candidate, state, error, tip_pos, tip_normal, mesh, &points, &hang, ctx, args,
            );
        }
        return PlaceOutcome::rejected(RejectReason::Trunk(error));
    }

    // No trunk whose cone lies within 15° of flat on the surface.
    if let Some(cone) = &trunk.trunk.contact_cone
        && is_side_wall_contact(
            cone.normal,
            live.tip.cone_angle_mode.unwrap_or_default(),
            live.tip.adaptive_cone_angle_offset_deg,
        )
    {
        return PlaceOutcome::rejected(RejectReason::SideWallContact);
    }

    let mut lattice_settings = live.clone();
    if !grid_on {
        lattice_settings.grid.enabled = false;
    }
    let decision = {
        let field = mesh.map(|m| m.field());
        decide_grid_placement(
            &DecideGridPlacementArgs {
                settings: &lattice_settings,
                snapshot: draft,
                candidate: &trunk,
                tip_pos,
                tip_normal,
                model_id,
                mesh: field.as_deref().map(|f| f as &dyn CollisionField),
            },
            live,
            &mut || ctx.id(),
        )
    };
    // A support whose tip cone would go into the model is turned down as the
    // lattice turns one down (#500).
    let decision = match decision {
        GridPlacementDecision::Place {
            node_key,
            placed,
            support_data,
        } if entity_cones(&placed.entity)
            .into_iter()
            .any(|cone| ctx.cone_crosses(cone)) =>
        {
            GridPlacementDecision::Reject {
                node_key,
                reason: GridPlacementRejectReason::CollisionWithModel,
                trunk_build: None,
                support_data,
            }
        }
        other => other,
    };

    let reason = match decision {
        GridPlacementDecision::Place { placed, .. } => {
            if let Some(host) = &placed.hosted_by
                && is_host_at_attachment_capacity(
                    host.type_id,
                    &host.id,
                    auto.max_attachments_per_trunk,
                    draft,
                )
            {
                return PlaceOutcome::rejected(RejectReason::GridRejectOther);
            }
            let kind = placed.type_id;
            let mut entity = placed.entity;
            if get_support_type_descriptor(kind).has_origin {
                let origin = if candidate.grid_point == Some(true) {
                    SupportOriginId::Overhang
                } else {
                    origin_by_source(candidate.source)
                };
                set_origin(&mut entity, origin);
            }
            let id = entity_id(&entity);
            draft_commit_support(state, kind, entity, &placed.supplied);
            return PlaceOutcome::placed(kind, id);
        }
        GridPlacementDecision::Reject { reason, .. } => match reason {
            GridPlacementRejectReason::CollisionWithModel => RejectReason::GridRejectCollision,
            GridPlacementRejectReason::NoValidAttachment
            | GridPlacementRejectReason::KnotAboveTip => RejectReason::GridRejectNoAttachment,
            _ => RejectReason::GridRejectOther,
        },
    };
    if !grid_on {
        return PlaceOutcome::rejected(reason);
    }
    if live.grid.pillars {
        // No node sees this tip: hang it from whatever stands near, pillar
        // or member, before raising anything new (#464).
        let reach = live.grid.spacing_mm * GRID_PARENT_REACH_NODES;
        if let Ok(hung) = hang(
            &near_points(),
            &no_hosts,
            "auto-parent",
            reach,
            reach,
            auto.max_attachments_per_trunk,
            &*state,
            origin_of(candidate.source),
            FanHostOrder::Nearest,
            None,
        ) {
            let mut out = hung_here(state, hung);
            out.grid_fallback = true;
            out.grid_fallback_reason = Some("parented");
            return out;
        }
    }
    // The lattice cannot serve this tip: once more, off the lattice.
    let mut off = place_one_candidate(candidate, state, args, ctx, true);
    if off.kind.is_some() {
        off.grid_fallback = true;
        off.grid_fallback_reason = Some(reason.as_str());
    }
    off
}

type Hang<'h> = dyn Fn(
        &[FanShaftPoint],
        &HashSet<String>,
        &str,
        f64,
        f64,
        f64,
        &SupportState,
        SupportOriginId,
        FanHostOrder,
        Option<f64>,
    ) -> Result<FanLeafResult, FanLeafResult>
    + 'h;

/// A contact whose trunk hits the model on its way down: hung from a support
/// nearby within any span; inside a closed cavity, hung from a stick a
/// neighbour stands on (#632); else a stick or twig from the model below
/// (none when supports stand on the plate alone, #471).
#[allow(clippy::too_many_arguments)]
fn bridged(
    candidate: &CandidatePoint,
    state: &mut SupportState,
    error: LimitationCode,
    tip_pos: Vec3,
    tip_normal: Vec3,
    mesh: &super::helpers::PlanMesh<'_>,
    near_points: &[FanShaftPoint],
    hang: &Hang<'_>,
    ctx: &PlanCtx<'_>,
    args: &PlaceArgs<'_>,
) -> PlaceOutcome {
    let live = ctx.live();
    let no_hosts = HashSet::new();
    let origin = origin_of(candidate.source);
    let refusal = match hang(
        near_points,
        args.grid_host_ids.unwrap_or(&no_hosts),
        "auto-cavity-fan",
        CAVITY_FAN_RADIUS_MM,
        CAVITY_FAN_RADIUS_MM,
        live.auto_support.max_attachments_per_trunk,
        state,
        origin,
        FanHostOrder::Steepest,
        Some(f64::INFINITY),
    ) {
        Ok(hung) => return hung_here(state, hung),
        Err(FanLeafResult::Refused {
            reason,
            nearest_host_mm,
            nearest_steep_mm,
            steep_angle_deg,
        }) => Some(match nearest_host_mm {
            None => reason.as_str().to_owned(),
            Some(nearest) => format!(
                "{} (nearest {}mm plan{})",
                reason.as_str(),
                js_to_fixed(nearest, 1),
                match nearest_steep_mm {
                    None => ", no legal host".to_owned(),
                    Some(steep) => format!(
                        ", nearest legal {}mm @ {}°",
                        js_to_fixed(steep, 1),
                        js_to_fixed(steep_angle_deg.unwrap_or(0.0), 0)
                    ),
                }
            ),
        }),
        Err(_) => None,
    };
    let refused = || PlaceOutcome {
        cavity_fan_refusal: refusal.clone(),
        ..PlaceOutcome::rejected(RejectReason::Trunk(error))
    };
    if live.auto_support.zone_rules.plate_only {
        return refused();
    }
    if ctx.is_enclosed() {
        let sticks = collect_stick_shaft_points_near(state, tip_pos, CAVITY_STICK_FAN_RADIUS_MM);
        if !sticks.is_empty()
            && let Ok(hung) = hang(
                &sticks,
                &no_hosts,
                "auto-cavity-stick",
                CAVITY_STICK_FAN_RADIUS_MM,
                CAVITY_STICK_FAN_RADIUS_MM,
                CAVITY_STICK_MAX_MEMBERS,
                state,
                origin,
                FanHostOrder::Nearest,
                None,
            )
        {
            return hung_here(state, hung);
        }
    }
    // A cavity is tight: the thinnest size of the preset, unless a hand
    // asked for one (#360, #564).
    let thinnest = || {
        live.auto_support
            .zone_rules
            .sizes
            .iter()
            .min_by(|a, b| a.trunk_diameter_mm.total_cmp(&b.trunk_diameter_mm))
            .map(|s| s.band())
    };
    let band = ctx
        .hand_zone_size()
        .map(|s| s.band())
        .or_else(thinnest)
        .unwrap_or_else(|| ctx.bands.active(live));
    let bridge_with = |band: &SizingBand| {
        let field = mesh.field();
        build_cavity_bridge(
            tip_pos,
            tip_normal,
            &candidate.model_id,
            ModelMesh {
                field: &*field,
                rays: mesh.rays(),
            },
            mesh.side,
            Some(CavityBridgeSizing {
                tip_contact_diameter_mm: band.tip_contact_diameter_mm,
                shaft_diameter_mm: band.shaft_diameter_mm,
            }),
            live,
            &mut || ctx.id(),
        )
    };
    // In a closed cavity a stick takes the contact's own cavity size, wider
    // and stiffer (#632); a twig, short and in a tight spot, stays thinnest.
    let Some(bridge) = ctx
        .enclosed_zone_size()
        .and_then(|size| bridge_with(&size.band()))
        .filter(|bridge| bridge.kind == SupportTypeId::Stick)
        .or_else(|| bridge_with(&band))
    else {
        return refused();
    };
    // The span from the upper contact, which the builder puts at the tip:
    // the cap has never turned one down, and is kept as a guard.
    let upper = contact_endpoints_for(bridge.kind)
        .into_iter()
        .find(|e| e.end == EndpointEnd::Upper)
        .map(|e| match (&bridge.entity, e.field) {
            (BridgeEntity::Twig(t), "contactDiskA") => t.contact_disk_a.pos,
            (BridgeEntity::Twig(t), _) => t.contact_disk_b.pos,
            (BridgeEntity::Stick(s), "contactConeA") => s.contact_cone_a.pos,
            (BridgeEntity::Stick(s), _) => s.contact_cone_b.pos,
        });
    if upper.is_some_and(|u| {
        js_hypot(&[tip_pos.x - u.x, tip_pos.y - u.y, tip_pos.z - u.z]) > MAX_CAVITY_BRIDGE_MM
    }) {
        return refused();
    }
    let entity = match bridge.entity {
        BridgeEntity::Twig(t) => SupportEntityValue::Twig(t),
        BridgeEntity::Stick(s) => SupportEntityValue::Stick(s),
    };
    let id = entity_id(&entity);
    draft_add_entity(state, bridge.kind, entity);
    PlaceOutcome {
        cavity_fan_refusal: refusal,
        ..PlaceOutcome::placed(bridge.kind, id)
    }
}

/// Adds a hung member to `state`, as placed.
fn hung_here(state: &mut SupportState, hung: FanLeafResult) -> PlaceOutcome {
    let FanLeafResult::Ok {
        kind,
        parent_knot,
        member,
        entity_id,
        ..
    } = hung
    else {
        unreachable!("only a hung member is added");
    };
    add_fanned(state, kind, *parent_knot, *member);
    PlaceOutcome::placed(kind, entity_id)
}

/// A member joined to a trunk: its knot and itself, for the caller to add in
/// that order.
struct Joined {
    parent_knot: Knot,
    kind: SupportTypeId,
    member: SupportEntityValue,
    id: String,
}

/// The longest member that joins a trunk, mm.
const MAX_JOIN_SPAN_MM: f64 = 12.0;

/// `candidate` joined to `host`'s highest shaft point that rises steeply
/// enough to the tip: a leaf over a short span, a branch over a long one.
fn join_trunk(
    candidate: &CandidatePoint,
    tip_pos: Vec3,
    tip_normal: Vec3,
    host: &MergeHost,
    draft: &SupportState,
    ctx: &PlanCtx<'_>,
) -> Option<Joined> {
    let live = ctx.live();
    let mesh = ctx.mesh.as_ref();
    let host_entity = host_entity_of(draft, host.host_type_id, &host.host_id);
    let least_rise = js_max(45.0, 90.0 - member_max_angle_from_vertical_deg(live));
    // Down each segment from its top every 0.5 mm (256 points at most), the
    // first point that serves.
    let mut best: Option<(Vec3, String, f64)> = None;
    for seg in host_entity.iter().flat_map(|e| &e.segments) {
        let Some((start, end)) = host_segment_span(draft, host_entity?, seg) else {
            continue;
        };
        let span = js_hypot(&[end.x - start.x, end.y - start.y, end.z - start.z]);
        let steps = js_max(1.0, js_min(256.0, (span / 0.5).ceil()));
        let mut i = steps;
        while i >= 0.0 {
            let t = i / steps;
            i -= 1.0;
            let at = Vec3::new(
                start.x + (end.x - start.x) * t,
                start.y + (end.y - start.y) * t,
                start.z + (end.z - start.z) * t,
            );
            let rise = tip_pos.z - at.z;
            if rise <= 0.0 {
                continue;
            }
            let across = js_hypot(&[tip_pos.x - at.x, tip_pos.y - at.y]);
            if js_hypot(&[across, rise]) > MAX_JOIN_SPAN_MM
                || (js_atan2(rise, across) * 180.0) / std::f64::consts::PI < least_rise
            {
                continue;
            }
            if best.as_ref().is_none_or(|(p, _, _)| at.z > p.z) {
                best = Some((at, seg.id.clone(), t));
            }
            break;
        }
    }
    let (knot_pos, seg_id, knot_t) = best?;
    let host_diameter = host_entity
        .and_then(|e| e.segments.iter().find(|s| s.id == seg_id))
        .map(|s| s.diameter)
        .filter(|&d| d != 0.0 && !d.is_nan())
        .unwrap_or(1.0);
    let parent_knot = Knot {
        id: free_knot_id(draft, &format!("auto-merge-{}", candidate.id)),
        parent_shaft_id: if seg_id.is_empty() {
            host.host_id.clone()
        } else {
            seg_id
        },
        t: Some(knot_t),
        pos: knot_pos,
        // A shaft joint's own size.
        diameter: Some(host_diameter + 0.125),
    };
    let (dx, dy, dz) = (
        tip_pos.x - knot_pos.x,
        tip_pos.y - knot_pos.y,
        tip_pos.z - knot_pos.z,
    );
    let span = (dx * dx + dy * dy + dz * dz).sqrt();
    let cap = live.auto_support.max_attachments_per_trunk;
    let origin = if candidate.source == CandidateSource::Overhang {
        SupportOriginId::Overhang
    } else {
        SupportOriginId::Island
    };
    let across = (dx * dx + dy * dy).sqrt();
    let rise = tip_pos.z - knot_pos.z;
    let (kind, member, id) = if span <= MAX_LEAF_SPAN_BEFORE_BRANCH_MM {
        // A leaf has to rise at least 1.5 mm; else a trunk of its own.
        if rise <= 0.0 || rise < 1.5 {
            return None;
        }
        let built = {
            let field = mesh.map(|m| m.field());
            build_leaf_data(
                &LeafBuildInput {
                    tip_pos,
                    surface_normal: tip_normal,
                    model_id: &candidate.model_id,
                    parent_knot: &parent_knot,
                    // The cone's body is the host's shaft.
                    host_diameter_mm: host_diameter,
                    tip_contact_diameter_mm: Some(ctx.bands.active(live).tip_contact_diameter_mm),
                    mesh: field.as_deref().map(|f| f as &dyn CollisionField),
                },
                live,
                &mut || ctx.id(),
            )
        };
        if built.support_data.error.is_some() {
            return None;
        }
        let cone = &built.leaf.contact_cone;
        if mesh.is_some_and(|m| {
            contact_cone_collides(
                parent_knot.pos,
                cone.pos,
                cone.normal,
                cone.surface_normal,
                m,
            )
        }) || ctx.cone_crosses(cone)
            || is_host_at_attachment_capacity(host.host_type_id, &host.host_id, cap, draft)
        {
            return None;
        }
        let mut leaf = built.leaf;
        leaf.base.origin = Some(origin);
        let kind = leaf.base.type_id.expect("a built member has its type");
        let id = leaf.base.id.clone();
        (kind, SupportEntityValue::Leaf(leaf), id)
    } else {
        if (js_atan2(across, rise) * 180.0) / std::f64::consts::PI
            > js_min(50.0, member_max_angle_from_vertical_deg(live))
        {
            return None;
        }
        let band = ctx.bands.active(live);
        let built = build_hosted_branch(
            &HostedBranchInput {
                tip_pos,
                tip_normal,
                model_id: &candidate.model_id,
                parent_knot: &parent_knot,
                tip_contact_diameter_mm: Some(band.tip_contact_diameter_mm),
                roots_diameter_mm: Some(band.root_diameter_mm),
            },
            band.shaft_diameter_mm,
            host_diameter,
            ctx,
            &|attempt| {
                attempt.support_data.error.is_some()
                    || mesh.is_some_and(|m| branch_collides_with_sdf(&attempt.branch.segments, m))
            },
        )?;
        if branch_departure_angle_deg(&built.branch, knot_pos)
            > member_max_angle_from_vertical_deg(live)
            || is_host_at_attachment_capacity(host.host_type_id, &host.host_id, cap, draft)
        {
            return None;
        }
        let mut branch = built.branch;
        branch.base.origin = Some(origin);
        let kind = branch.base.type_id.expect("a built member has its type");
        let id = branch.base.id.clone();
        (kind, SupportEntityValue::Branch(branch), id)
    };
    Some(Joined {
        parent_knot,
        kind,
        member,
        id,
    })
}

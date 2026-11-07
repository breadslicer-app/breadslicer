//! DragonFruit `src/supports/autoSupport/autoPlace.ts`, `placeOneCandidate`:
//! one candidate through the placement pipeline — surface normal, fan or
//! gridless merge, sized trunk, cavity fallback, side-wall guard, grid
//! decision, commit — the same sequence a manual click runs.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::fan::{
    FanArgs, FanHostOrder, FanLeafRefusal, FanLeafResult, add_fanned, fan_leaf_to_host,
};
use super::helpers::{
    FanShaftPoint, HostedBranchInput, PlanCtx, branch_collides_with_sdf,
    branch_departure_angle_deg, build_hosted_branch, collect_fan_shaft_points,
    collect_fan_shaft_points_near, contact_cone_collides, find_merge_host, free_knot_id,
    host_entity_of, host_segment_span, is_host_at_attachment_capacity,
    member_max_angle_from_vertical_deg, origin_by_source,
};
use crate::supports::auto_support::constants::{
    CAVITY_FAN_RADIUS_MM, GRID_HOST_FAN_RADIUS_MM, MAX_CAVITY_BRIDGE_MM,
    MAX_LEAF_SPAN_BEFORE_BRANCH_MM, MIN_LEAF_FAN_RADIUS_MM,
};
use crate::supports::auto_support::parameter_sizing::{
    ModelSizingContext, preset_for_area, size_parameters,
};
use crate::supports::auto_support::settings::SizingPreset;
use crate::supports::auto_support::support_draft::{
    draft_add_entity, draft_add_primitive, draft_commit_support,
};
use crate::supports::auto_support::types::{CandidatePoint, CandidateSource};
use crate::supports::model_mesh::ModelMesh;
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::placement_logic::cone_axis_policy::is_side_wall_contact;
use crate::supports::placement_logic::grid::grid_placement::{
    DecideGridPlacementArgs, GridPlacementDecision, GridPlacementRejectReason,
    decide_grid_placement,
};
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_type_registry::{
    CONTACT_OVERRIDE_REGISTRATIONS, EndpointEnd, PlacementPrimitive, SupportEntityValue,
    SupportOriginId, SupportPlacementMetric, SupportPlacementSettingPath, SupportTypeId,
    contact_endpoints_for, get_support_type_descriptor, select_type_for_placement,
};
use crate::supports::support_types::contact_bridge::BridgeEntity;
use crate::supports::support_types::leaf::leaf_builder::{LeafBuildInput, build_leaf_data};
use crate::supports::support_types::trunk::cavity_bridge::{
    CavityBridgeSizing, build_cavity_bridge,
};
use crate::supports::support_types::trunk::trunk_builder::{
    TrunkBuildInput, TrunkBuildResult, TrunkOverrides, TrunkRouter, build_trunk_data,
    build_trunk_data_routed,
};
use crate::supports::support_types::trunk::trunk_route_resolution::get_resolved_snapped_root_pos;
use crate::supports::types::{Knot, LimitationCode, SupportState, Vec3};
use crate::three::math_utils::{js_atan2, js_hypot, js_max, js_min, js_to_fixed};

/// Why a candidate was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// The trunk could not be built, and why.
    Trunk(LimitationCode),
    /// The trunk's contact cone would sit on a side wall (within 15° of flat).
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

/// What one candidate resolved to. `kind: None` is `'reject'`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceOutcome {
    pub kind: Option<SupportTypeId>,
    pub rejected_reason: Option<RejectReason>,
    pub preset: Option<SizingPreset>,
    pub entity_id: Option<String>,
    pub fan_refusal: Option<FanLeafRefusal>,
    pub merge_refusal: Option<&'static str>,
    pub cavity_fan_refusal: Option<String>,
    pub grid_fallback: bool,
    /// Why the lattice could not serve this candidate (#463).
    pub grid_fallback_reason: Option<&'static str>,
}

impl PlaceOutcome {
    fn placed(kind: SupportTypeId, preset: SizingPreset, entity_id: String) -> Self {
        PlaceOutcome {
            kind: Some(kind),
            rejected_reason: None,
            preset: Some(preset),
            entity_id: Some(entity_id),
            fan_refusal: None,
            merge_refusal: None,
            cavity_fan_refusal: None,
            grid_fallback: false,
            grid_fallback_reason: None,
        }
    }

    fn rejected(reason: RejectReason, preset: SizingPreset) -> Self {
        PlaceOutcome {
            kind: None,
            rejected_reason: Some(reason),
            preset: Some(preset),
            entity_id: None,
            fan_refusal: None,
            merge_refusal: None,
            cavity_fan_refusal: None,
            grid_fallback: false,
            grid_fallback_reason: None,
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

/// `candidate.source as SupportOrigin`, the cast the cavity fan makes.
fn source_as_origin(source: CandidateSource) -> SupportOriginId {
    match source {
        CandidateSource::Voxel => SupportOriginId::Voxel,
        CandidateSource::Minima => SupportOriginId::Minima,
        CandidateSource::Intersection => SupportOriginId::Intersection,
        CandidateSource::Overhang => SupportOriginId::Overhang,
        CandidateSource::Stabilization => SupportOriginId::Stabilization,
        CandidateSource::Reinforcement => SupportOriginId::Reinforcement,
    }
}

/// The run-wide placement inputs.
pub struct PlaceArgs<'a> {
    pub grid_host_ids: Option<&'a HashSet<String>>,
    pub sizing_ctx: Option<&'a ModelSizingContext>,
}

/// Build the sized trunk for a contact, routed when there is a mesh.
fn build_sized_trunk(input: &TrunkBuildInput<'_>, ctx: &PlanCtx<'_>) -> TrunkBuildResult {
    match &ctx.mesh {
        Some(mesh) => {
            let mut sdf = mesh.sdf.borrow_mut();
            let mut stats = ctx.stats.borrow_mut();
            build_trunk_data_routed(
                input,
                &mut TrunkRouter {
                    sdf: &mut sdf,
                    matrix_world: mesh.matrix,
                    stats: &mut stats,
                },
                ctx.live(),
                &mut || ctx.id(),
            )
        }
        None => build_trunk_data(input, ctx.live(), &mut || ctx.id()).expect("mesh-free build"),
    }
}

/// How far a tip no node sees looks for something to hang from, in grid steps.
const GRID_PARENT_REACH_NODES: f64 = 1.5;

/// How high `candidate` joins the pillar of its grid node: where the 45 degree
/// run from its tip meets that node's vertical. `None` when no node serves it
/// (#464).
pub(super) fn grid_join_height(
    candidate: &CandidatePoint,
    sizing_ctx: Option<&ModelSizingContext>,
    ctx: &PlanCtx<'_>,
) -> Option<f64> {
    let live = ctx.live();
    let (tip_pos, tip_normal) =
        if candidate.grid_point == Some(true) && candidate.tip_normal.z < 0.0 {
            (candidate.tip_pos, candidate.tip_normal)
        } else {
            ctx.contact_at(candidate.tip_pos)
        };
    let overrides = size_parameters(
        candidate,
        live.auto_support.size_scale,
        sizing_ctx,
        live,
        ctx.bands,
    );
    let built = build_sized_trunk(
        &TrunkBuildInput {
            tip_pos,
            tip_normal,
            model_id: &candidate.model_id,
            mesh: None,
            is_preview: Some(true),
            overrides: Some(TrunkOverrides {
                roots_diameter_mm: overrides.roots_diameter_mm,
                roots_disk_height_mm: overrides.roots_disk_height_mm,
                roots_cone_height_mm: overrides.roots_cone_height_mm,
                shaft_diameter_mm: overrides.shaft_diameter_mm,
                tip_contact_diameter_mm: overrides.tip_contact_diameter_mm,
                tip_body_diameter_mm: overrides.tip_body_diameter_mm,
                tip_length_mm: overrides.tip_length_mm,
                tip_disk_length_override_mm: None,
                tip_penetration_mm: overrides.tip_penetration_mm,
            }),
            is_small_island: None,
            ignore_grid: Some(false),
        },
        ctx,
    );
    if built.error.is_some() {
        return None;
    }
    let root = get_resolved_snapped_root_pos(&built.route, built.root.transform.pos);
    Some(tip_pos.z - (tip_pos.x - root.x).hypot(tip_pos.y - root.y))
}

/// One candidate through the placement pipeline. `ignore_grid` is set only by
/// the grid-fallback retry below.
///
/// SoulCrafted: the placement is made in `state` itself, which a rejection
/// leaves untouched, instead of in a copy of the whole state per candidate.
pub fn place_one_candidate(
    candidate: &CandidatePoint,
    state: &mut SupportState,
    args: &PlaceArgs<'_>,
    ctx: &PlanCtx<'_>,
    ignore_grid: bool,
) -> PlaceOutcome {
    let live: &SupportSettings = ctx.live();
    let auto = &live.auto_support;
    let grid_enabled = live.grid.enabled && !ignore_grid;
    let mesh = ctx.mesh.as_ref();
    let draft: &SupportState = state;

    // Grid points carry the region's exact surface position and normal.
    let (tip_pos, tip_normal) =
        if candidate.grid_point == Some(true) && candidate.tip_normal.z < 0.0 {
            (candidate.tip_pos, candidate.tip_normal)
        } else {
            ctx.contact_at(candidate.tip_pos)
        };

    let preset = preset_for_area(candidate.island_area_mm2);
    let fan_max_angle = js_min(
        auto.leaf_fan_max_angle_deg,
        member_max_angle_from_vertical_deg(live),
    );
    // `auto.leafFanRadiusMm ?? LEAF_FAN_RADIUS_MM`: the Rust field is always set.
    let fan_radius = js_max(MIN_LEAF_FAN_RADIUS_MM, auto.leaf_fan_radius_mm);
    let mut fan_pool: Option<Vec<FanShaftPoint>> = None;
    let mut fan_points = || -> Vec<FanShaftPoint> {
        fan_pool
            .get_or_insert_with(|| {
                if ctx.near_fans() {
                    let reach = fan_radius
                        .max(GRID_HOST_FAN_RADIUS_MM)
                        .max(CAVITY_FAN_RADIUS_MM);
                    collect_fan_shaft_points_near(draft, tip_pos, reach)
                } else {
                    collect_fan_shaft_points(draft)
                }
            })
            .clone()
    };
    let fan_ok = |fan: FanLeafResult| -> Result<FanLeafResult, FanLeafResult> {
        match fan {
            FanLeafResult::Ok { .. } => Ok(fan),
            refused => Err(refused),
        }
    };

    // ── Gridless merge check ──────────────────────────────────────
    let mut merge_host_found = false;
    let mut fan_refusal: Option<FanLeafRefusal> = None;
    if !grid_enabled
        && candidate.grid_point == Some(true)
        && candidate.source == CandidateSource::Overhang
        && !candidate.id.starts_with("grid-")
        && let Some(grid_host_ids) = args.grid_host_ids
    {
        // Organic grid points fan into ISLAND trunks only.
        let island_pool: Vec<FanShaftPoint> = fan_points()
            .into_iter()
            .filter(|sp| !grid_host_ids.contains(&sp.host_id))
            .collect();
        if !island_pool.is_empty() {
            let fan = fan_leaf_to_host(
                &FanArgs {
                    target: tip_pos,
                    model_id: &candidate.model_id,
                    shaft_points: &island_pool,
                    grid_host_ids: &HashSet::new(),
                    knot_id_prefix: &format!("auto-fan-{}", candidate.id),
                    fan_radius_mm: fan_radius,
                    grid_host_fan_radius_mm: GRID_HOST_FAN_RADIUS_MM,
                    max_angle_deg: fan_max_angle,
                    max_attachments: auto.max_attachments_per_trunk,
                    draft,
                    origin: Some(SupportOriginId::Overhang),
                    host_order: FanHostOrder::Steepest,
                    rescue_span_override_mm: None,
                },
                ctx,
            );
            if let Ok(fanned) = fan_ok(fan) {
                return placed_fan(state, preset, fanned);
            }
        }
    }
    // Stabilization anchors stay standalone.
    if !grid_enabled
        && candidate.grid_point != Some(true)
        && candidate.source != CandidateSource::Stabilization
    {
        if candidate.source == CandidateSource::Overhang
            && let Some(grid_host_ids) = args.grid_host_ids
        {
            let pool = fan_points();
            let fan = fan_leaf_to_host(
                &FanArgs {
                    target: tip_pos,
                    model_id: &candidate.model_id,
                    shaft_points: &pool,
                    grid_host_ids,
                    knot_id_prefix: &format!("auto-fan-{}", candidate.id),
                    fan_radius_mm: fan_radius,
                    grid_host_fan_radius_mm: GRID_HOST_FAN_RADIUS_MM,
                    max_angle_deg: fan_max_angle,
                    max_attachments: auto.max_attachments_per_trunk,
                    draft,
                    origin: Some(SupportOriginId::Overhang),
                    host_order: FanHostOrder::Steepest,
                    rescue_span_override_mm: None,
                },
                ctx,
            );
            match fan_ok(fan) {
                Ok(fanned) => return placed_fan(state, preset, fanned),
                Err(FanLeafResult::Refused { reason, .. }) => fan_refusal = Some(reason),
                Err(_) => {}
            }
        }
        let mut host = find_merge_host(tip_pos, &candidate.model_id, draft);
        // Island candidates never merge INTO grid trunks.
        if host
            .as_ref()
            .is_some_and(|h| args.grid_host_ids.is_some_and(|g| g.contains(&h.host_id)))
        {
            host = None;
        }
        if let Some(host) = host {
            merge_host_found = true;
            if let Some(merge) = try_merge(candidate, tip_pos, tip_normal, &host, draft, ctx) {
                draft_add_primitive(state, PlacementPrimitive::Knot(merge.parent_knot));
                draft_add_entity(state, merge.kind, merge.member);
                return PlaceOutcome::placed(merge.kind, preset, merge.id);
            }
        }
    }

    // Empirical sizing from the candidate's own island area.
    let overrides = size_parameters(candidate, auto.size_scale, args.sizing_ctx, live, ctx.bands);
    let is_small_island = (candidate.source != CandidateSource::Overhang
        && candidate.island_area_mm2 < 5.0)
        || candidate.z_height < 15.0;
    let trunk_input = TrunkBuildInput {
        tip_pos,
        tip_normal,
        model_id: &candidate.model_id,
        mesh: None,
        is_preview: Some(false),
        overrides: Some(TrunkOverrides {
            roots_diameter_mm: overrides.roots_diameter_mm,
            roots_disk_height_mm: overrides.roots_disk_height_mm,
            roots_cone_height_mm: overrides.roots_cone_height_mm,
            shaft_diameter_mm: overrides.shaft_diameter_mm,
            tip_contact_diameter_mm: overrides.tip_contact_diameter_mm,
            tip_body_diameter_mm: overrides.tip_body_diameter_mm,
            tip_length_mm: overrides.tip_length_mm,
            tip_disk_length_override_mm: None,
            tip_penetration_mm: overrides.tip_penetration_mm,
        }),
        is_small_island: Some(is_small_island),
        ignore_grid: Some(ignore_grid),
    };
    let trunk_result = build_sized_trunk(&trunk_input, ctx);

    // A near-plate type builds different geometry; its own validation decides.
    let read_setting = |path: SupportPlacementSettingPath| match path {
        SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm => {
            Some(live.mesh_to_mesh.stick_vs_twig_cutoff_mm)
        }
    };
    let claimed_type = (trunk_result.error == Some(LimitationCode::OutOfBounds))
        .then(|| {
            select_type_for_placement(SupportPlacementMetric::TipHeight, tip_pos.z, &read_setting)
        })
        .flatten();
    let defer_to_type_override =
        claimed_type.is_some_and(|t| CONTACT_OVERRIDE_REGISTRATIONS.contains(&t));
    if let Some(error) = trunk_result.error
        && !defer_to_type_override
    {
        // Cavity fallback: the trunk cannot reach the plate.
        if error == LimitationCode::CollisionWithModel
            && let Some(mesh) = mesh
        {
            let pool = fan_points();
            let fan = fan_leaf_to_host(
                &FanArgs {
                    target: tip_pos,
                    model_id: &candidate.model_id,
                    shaft_points: &pool,
                    grid_host_ids: args.grid_host_ids.unwrap_or(&HashSet::new()),
                    knot_id_prefix: &format!("auto-cavity-fan-{}", candidate.id),
                    fan_radius_mm: CAVITY_FAN_RADIUS_MM,
                    grid_host_fan_radius_mm: CAVITY_FAN_RADIUS_MM,
                    max_angle_deg: fan_max_angle,
                    max_attachments: auto.max_attachments_per_trunk,
                    draft,
                    origin: Some(source_as_origin(candidate.source)),
                    host_order: FanHostOrder::Steepest,
                    rescue_span_override_mm: Some(f64::INFINITY),
                },
                ctx,
            );
            let cavity_fan_refusal = match fan_ok(fan) {
                Ok(fanned) => return placed_fan(state, preset, fanned),
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
            // SoulCrafted (#360): a zone preset bridges a cavity with its
            // thinnest size, whatever the zone: a cavity is tight.
            let band = live
                .auto_support
                .zone_rules
                .as_ref()
                .and_then(|r| {
                    r.sizes
                        .iter()
                        .min_by(|a, b| a.trunk_diameter_mm.total_cmp(&b.trunk_diameter_mm))
                        .map(|s| s.band())
                })
                .unwrap_or_else(|| ctx.bands.active(live));
            let bridge = {
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
            if let Some(bridge) = bridge {
                // The cap measures from the `upper` contact, which the builder
                // sorts to the tip, so the span is ~0 and has never rejected
                // anything; kept as DragonFruit has it.
                let upper_field = contact_endpoints_for(bridge.kind)
                    .into_iter()
                    .find(|e| e.end == EndpointEnd::Upper)
                    .map(|e| e.field);
                let lower = upper_field.map(|f| match (&bridge.entity, f) {
                    (BridgeEntity::Twig(t), "contactDiskA") => t.contact_disk_a.pos,
                    (BridgeEntity::Twig(t), _) => t.contact_disk_b.pos,
                    (BridgeEntity::Stick(s), "contactConeA") => s.contact_cone_a.pos,
                    (BridgeEntity::Stick(s), _) => s.contact_cone_b.pos,
                });
                let bridge_len =
                    lower.map(|l| js_hypot(&[tip_pos.x - l.x, tip_pos.y - l.y, tip_pos.z - l.z]));
                if !bridge_len.is_some_and(|len| len > MAX_CAVITY_BRIDGE_MM) {
                    let entity = match bridge.entity {
                        BridgeEntity::Twig(t) => SupportEntityValue::Twig(t),
                        BridgeEntity::Stick(s) => SupportEntityValue::Stick(s),
                    };
                    let id = entity_id(&entity);
                    draft_add_entity(state, bridge.kind, entity);
                    let mut out = PlaceOutcome::placed(bridge.kind, preset, id);
                    out.cavity_fan_refusal = cavity_fan_refusal;
                    return out;
                }
            }
        }
        return PlaceOutcome::rejected(RejectReason::Trunk(error), preset);
    }

    // Side-wall guard: no trunk whose contact cone lies within 15° of flat.
    if let Some(cone) = &trunk_result.trunk.contact_cone {
        let n = cone.normal;
        if is_side_wall_contact(
            n,
            live.tip.cone_angle_mode.unwrap_or_default(),
            live.tip.adaptive_cone_angle_offset_deg,
        ) {
            return PlaceOutcome::rejected(RejectReason::SideWallContact, preset);
        }
    }

    // The standard grid placement engine.
    let mut decision_settings = live.clone();
    if !grid_enabled {
        decision_settings.grid.enabled = false;
    }
    let decision = {
        let field = mesh.map(|m| m.field());
        decide_grid_placement(
            &DecideGridPlacementArgs {
                settings: &decision_settings,
                snapshot: draft,
                candidate: &trunk_result,
                tip_pos,
                tip_normal,
                model_id: &candidate.model_id,
                mesh: field.as_deref().map(|f| f as &dyn CollisionField),
                is_preview: None,
            },
            live,
            &mut || ctx.id(),
        )
    };

    match decision {
        GridPlacementDecision::Place { placed, .. } => {
            if let Some(hosted_by) = &placed.hosted_by
                && is_host_at_attachment_capacity(
                    hosted_by.type_id,
                    &hosted_by.id,
                    auto.max_attachments_per_trunk,
                    draft,
                )
            {
                return PlaceOutcome::rejected(RejectReason::GridRejectOther, preset);
            }
            let type_id = placed.type_id;
            let mut entity = placed.entity;
            if get_support_type_descriptor(type_id).has_origin {
                set_origin(
                    &mut entity,
                    if candidate.grid_point == Some(true) {
                        SupportOriginId::Overhang
                    } else {
                        origin_by_source(candidate.source)
                    },
                );
            }
            let id = entity_id(&entity);
            draft_commit_support(state, type_id, entity, &placed.supplied);
            let merge_checked = !grid_enabled && candidate.grid_point != Some(true);
            let mut out = PlaceOutcome::placed(type_id, preset, id);
            out.fan_refusal = fan_refusal;
            out.merge_refusal = merge_checked.then_some(if merge_host_found {
                "rejected"
            } else {
                "noHost"
            });
            out
        }
        GridPlacementDecision::Reject { reason, .. } => {
            let mapped = match reason {
                GridPlacementRejectReason::CollisionWithModel => RejectReason::GridRejectCollision,
                GridPlacementRejectReason::NoValidAttachment
                | GridPlacementRejectReason::KnotAboveTip => RejectReason::GridRejectNoAttachment,
                _ => RejectReason::GridRejectOther,
            };
            if grid_enabled && live.grid.pillars {
                // No node sees this tip: hang it from whatever stands near,
                // pillar or member, before raising anything new (#464).
                let pool = fan_points();
                let reach = live.grid.spacing_mm * GRID_PARENT_REACH_NODES;
                let fan = fan_leaf_to_host(
                    &FanArgs {
                        target: tip_pos,
                        model_id: &candidate.model_id,
                        shaft_points: &pool,
                        grid_host_ids: &HashSet::new(),
                        knot_id_prefix: &format!("auto-parent-{}", candidate.id),
                        fan_radius_mm: reach,
                        grid_host_fan_radius_mm: reach,
                        max_angle_deg: fan_max_angle,
                        max_attachments: auto.max_attachments_per_trunk,
                        draft: &*state,
                        origin: Some(source_as_origin(candidate.source)),
                        host_order: FanHostOrder::Nearest,
                        rescue_span_override_mm: None,
                    },
                    ctx,
                );
                if let Ok(fanned) = fan_ok(fan) {
                    let mut out = placed_fan(state, preset, fanned);
                    out.grid_fallback = true;
                    out.grid_fallback_reason = Some("parented");
                    return out;
                }
            }
            if grid_enabled {
                // The lattice could not serve this tip: retry with the grid
                // off for this candidate alone.
                let mut fallback = place_one_candidate(candidate, state, args, ctx, true);
                if fallback.kind.is_some() {
                    fallback.grid_fallback = true;
                    fallback.grid_fallback_reason = Some(mapped.as_str());
                }
                return fallback;
            }
            PlaceOutcome::rejected(mapped, preset)
        }
    }
}

/// Adds an accepted fan to `state` and reports it as placed.
fn placed_fan(state: &mut SupportState, preset: SizingPreset, fan: FanLeafResult) -> PlaceOutcome {
    let FanLeafResult::Ok {
        kind,
        parent_knot,
        member,
        entity_id,
        ..
    } = fan
    else {
        unreachable!("only an accepted fan is placed");
    };
    add_fanned(state, kind, *parent_knot, *member);
    PlaceOutcome::placed(kind, preset, entity_id)
}

/// A merge [`try_merge`] found: the knot on the host and the member hung
/// from it, for the caller to add to the draft in that order.
struct Merge {
    parent_knot: Knot,
    kind: SupportTypeId,
    member: SupportEntityValue,
    id: String,
}

/// The gridless merge onto `host`: a leaf for a short span, a branch for a
/// long one, at the highest shaft sample that clears the steep minimum.
fn try_merge(
    candidate: &CandidatePoint,
    tip_pos: Vec3,
    tip_normal: Vec3,
    host: &super::helpers::MergeHost,
    draft: &SupportState,
    ctx: &PlanCtx<'_>,
) -> Option<Merge> {
    let live = ctx.live();
    let mesh = ctx.mesh.as_ref();
    let host_entity = host_entity_of(draft, host.host_type_id, &host.host_id);
    let steep_min_rise_deg = js_max(45.0, 90.0 - member_max_angle_from_vertical_deg(live));
    const MAX_MERGE_ATTACH_SPAN_MM: f64 = 12.0;
    const ATTACH_SEARCH_STEP_MM: f64 = 0.5;
    const MAX_ATTACH_SAMPLES: f64 = 256.0;
    let mut best: Option<(Vec3, String, f64)> = None;
    if let Some(entity) = host_entity {
        for seg in &entity.segments {
            let Some((start, end)) = host_segment_span(draft, entity, seg) else {
                continue;
            };
            let span_mm = js_hypot(&[end.x - start.x, end.y - start.y, end.z - start.z]);
            let steps = js_max(
                1.0,
                js_min(MAX_ATTACH_SAMPLES, (span_mm / ATTACH_SEARCH_STEP_MM).ceil()),
            );
            let mut i = steps;
            while i >= 0.0 {
                let t = i / steps;
                i -= 1.0;
                let sx = start.x + (end.x - start.x) * t;
                let sy = start.y + (end.y - start.y) * t;
                let sz = start.z + (end.z - start.z) * t;
                let v_dist = tip_pos.z - sz;
                if v_dist <= 0.0 {
                    continue;
                }
                let h_dist = js_hypot(&[tip_pos.x - sx, tip_pos.y - sy]);
                if js_hypot(&[h_dist, v_dist]) > MAX_MERGE_ATTACH_SPAN_MM {
                    continue;
                }
                let rise_deg = (js_atan2(v_dist, h_dist) * 180.0) / std::f64::consts::PI;
                if rise_deg < steep_min_rise_deg {
                    continue;
                }
                if best.as_ref().is_none_or(|(p, _, _)| sz > p.z) {
                    best = Some((Vec3::new(sx, sy, sz), seg.id.clone(), t));
                }
                // First legal sample from the top is the highest it offers.
                break;
            }
        }
    }
    let (knot_pos, seg_id, knot_t) = best?;
    let knot_diameter = host_entity
        .and_then(|e| e.segments.iter().find(|s| s.id == seg_id))
        .map(|s| s.diameter)
        .filter(|&dia| dia != 0.0 && !dia.is_nan())
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
        // Renders at exactly the trunk-joint size when unselected.
        diameter: Some(knot_diameter + 0.125),
        ..Knot::default()
    };
    let leaf_span_mm = ((tip_pos.x - knot_pos.x) * (tip_pos.x - knot_pos.x)
        + (tip_pos.y - knot_pos.y) * (tip_pos.y - knot_pos.y)
        + (tip_pos.z - knot_pos.z) * (tip_pos.z - knot_pos.z))
        .sqrt();
    let cap = live.auto_support.max_attachments_per_trunk;
    let member_origin = if candidate.source == CandidateSource::Overhang {
        SupportOriginId::Overhang
    } else {
        SupportOriginId::Island
    };
    let h_dist = ((tip_pos.x - knot_pos.x) * (tip_pos.x - knot_pos.x)
        + (tip_pos.y - knot_pos.y) * (tip_pos.y - knot_pos.y))
        .sqrt();
    let v_dist = tip_pos.z - knot_pos.z;
    if leaf_span_mm <= MAX_LEAF_SPAN_BEFORE_BRANCH_MM {
        // Too shallow (or above) falls through to a trunk; a short span is a leaf.
        if v_dist <= 0.0 || v_dist < 1.5 {
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
                    // Cone body = the HOST shaft, not the knot.
                    host_diameter_mm: knot_diameter,
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
        if let Some(mesh) = mesh
            && contact_cone_collides(
                parent_knot.pos,
                cone.pos,
                cone.normal,
                cone.surface_normal,
                mesh,
            )
        {
            return None;
        }
        if is_host_at_attachment_capacity(host.host_type_id, &host.host_id, cap, draft) {
            return None;
        }
        let mut leaf = built.leaf;
        leaf.base.origin = Some(member_origin);
        let kind = leaf
            .base
            .type_id
            .expect("a built member carries its typeId");
        let id = leaf.base.id.clone();
        return Some(Merge {
            parent_knot,
            kind,
            member: SupportEntityValue::Leaf(leaf),
            id,
        });
    }

    // A long span is a branch, for every origin.
    let merge_angle_deg = (js_atan2(h_dist, v_dist) * 180.0) / std::f64::consts::PI;
    if merge_angle_deg > js_min(50.0, member_max_angle_from_vertical_deg(live)) {
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
        knot_diameter,
        ctx,
        &|attempt| {
            attempt.support_data.error.is_some()
                || mesh.is_some_and(|m| branch_collides_with_sdf(&attempt.branch.segments, m))
        },
    )?;
    if branch_departure_angle_deg(&built.branch, knot_pos)
        > member_max_angle_from_vertical_deg(live)
    {
        return None;
    }
    if is_host_at_attachment_capacity(host.host_type_id, &host.host_id, cap, draft) {
        return None;
    }
    let mut branch = built.branch;
    branch.base.origin = Some(member_origin);
    let kind = branch
        .base
        .type_id
        .expect("a built member carries its typeId");
    let id = branch.base.id.clone();
    Some(Merge {
        parent_knot,
        kind,
        member: SupportEntityValue::Branch(branch),
        id,
    })
}

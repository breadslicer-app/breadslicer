//! DragonFruit `src/supports/autoSupport/autoPlace.ts`, the fan: carrying a
//! tip as a leaf or branch on a nearby host (`fanLeafToHost`) and the
//! consolidation branch (`buildConsolidationBranch`).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::helpers::{
    FanShaftPoint, HostedBranchInput, PlanCtx, branch_collides_with_sdf,
    branch_departure_angle_deg, build_hosted_branch, contact_cone_collides, free_knot_id,
    host_entity_of, is_host_at_attachment_capacity, leaf_path_crosses_supports,
    member_max_angle_from_vertical_deg,
};
use crate::supports::auto_support::constants::{
    FAN_LINK_TIP_INSET_MM, MAX_LEAF_SPAN_BEFORE_BRANCH_MM,
};
use crate::supports::auto_support::support_draft::{draft_add_entity, draft_add_primitive};
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::support_type_registry::{
    PlacementPrimitive, SupportEntityValue, SupportOriginId, SupportTypeId,
};
use crate::supports::support_types::leaf::leaf_builder::{LeafBuildInput, build_leaf_data};
use crate::supports::types::{Knot, SupportState, Vec3};
use crate::three::math_utils::{js_atan2, js_hypot, js_min, js_sin};

/// Why a fan-leaf attempt was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FanLeafRefusal {
    NoHost,
    SameZ,
    Angle,
    Blocked,
    Build,
    Cross,
    Capacity,
}

impl FanLeafRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            FanLeafRefusal::NoHost => "noHost",
            FanLeafRefusal::SameZ => "sameZ",
            FanLeafRefusal::Angle => "angle",
            FanLeafRefusal::Blocked => "blocked",
            FanLeafRefusal::Build => "build",
            FanLeafRefusal::Cross => "cross",
            FanLeafRefusal::Capacity => "capacity",
        }
    }
}

/// How a fanning/cluster link picks its host among eligible shaft samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanHostOrder {
    Steepest,
    Nearest,
}

/// Where a fan attempt attached, or why it did not.
#[derive(Debug, Clone, PartialEq)]
pub enum FanLeafResult {
    /// The knot on the host and the member hung from it, for the caller to
    /// add to the draft the fan was asked on ([`add_fanned`]).
    Ok {
        kind: SupportTypeId,
        parent_knot: Box<Knot>,
        member: Box<SupportEntityValue>,
        host_type_id: SupportTypeId,
        host_id: String,
        entity_id: String,
        dist_mm: f64,
        angle_deg: f64,
    },
    Refused {
        reason: FanLeafRefusal,
        /// Plan distance to the closest same-model host sample.
        nearest_host_mm: Option<f64>,
        /// Plan distance and angle of the closest sample that cleared the angle gate.
        nearest_steep_mm: Option<f64>,
        steep_angle_deg: Option<f64>,
    },
}

/// The fan's tuning, as DragonFruit's positional arguments.
pub struct FanArgs<'a> {
    pub target: Vec3,
    pub model_id: &'a str,
    pub shaft_points: &'a [FanShaftPoint],
    pub grid_host_ids: &'a HashSet<String>,
    pub knot_id_prefix: &'a str,
    pub fan_radius_mm: f64,
    pub grid_host_fan_radius_mm: f64,
    pub max_angle_deg: f64,
    pub max_attachments: f64,
    pub draft: &'a SupportState,
    pub origin: Option<SupportOriginId>,
    pub host_order: FanHostOrder,
    /// Span ceiling for the wide tier, overriding `reach / sin(maxAngle)`.
    pub rescue_span_override_mm: Option<f64>,
}

#[derive(Clone)]
struct FanCandidate<'p> {
    sp: &'p FanShaftPoint,
    dist2: f64,
    angle_deg: f64,
}

fn knot_for(sp: &FanShaftPoint, id: String) -> Knot {
    Knot {
        id,
        parent_shaft_id: sp.segment_id.clone().unwrap_or_else(|| sp.host_id.clone()),
        t: sp.t,
        pos: sp.pos,
        diameter: Some(sp.diameter + 0.125),
        ..Knot::default()
    }
}

/// Carry a tip as a leaf (short span) or a branch on the steepest (or nearest)
/// legal host sample in reach, with a wider rescue tier when none is.
/// Adds what a fan found to `draft`, the state it was asked on: the knot, then
/// the member. SoulCrafted: DragonFruit returns a copy of the whole draft with
/// both added; the caller adding them is the same state without the copy.
pub fn add_fanned(
    draft: &mut SupportState,
    kind: SupportTypeId,
    parent_knot: Knot,
    member: SupportEntityValue,
) {
    draft_add_primitive(draft, PlacementPrimitive::Knot(parent_knot));
    draft_add_entity(draft, kind, member);
}

pub fn fan_leaf_to_host(args: &FanArgs<'_>, ctx: &PlanCtx<'_>) -> FanLeafResult {
    let FanArgs {
        target,
        model_id,
        draft,
        max_angle_deg,
        ..
    } = *args;
    let mesh = ctx.mesh.as_ref();
    let mut candidates: Vec<FanCandidate<'_>> = Vec::new();
    let mut rescue: Vec<FanCandidate<'_>> = Vec::new();
    let mut refusal = FanLeafRefusal::NoHost;
    let mut nearest_host_mm = f64::INFINITY;
    let mut nearest_steep_mm = f64::INFINITY;
    let mut steep_angle_deg = f64::INFINITY;

    // The widest reach a sample can be taken at.
    let reach = args.fan_radius_mm.max(args.grid_host_fan_radius_mm);
    for sp in args.shaft_points {
        let ddx = sp.pos.x - target.x;
        let ddy = sp.pos.y - target.y;
        let ddz = sp.pos.z - target.z;
        let lateral2 = ddx * ddx + ddy * ddy;
        // SoulCrafted: a sample out of reach that is no nearer than the
        // nearest host changes nothing below; skip it before the host
        // lookups (the pool holds every host's samples, the lookups hash).
        if lateral2 > reach * reach && lateral2.sqrt() >= nearest_host_mm {
            continue;
        }
        if host_entity_of(draft, sp.host_type_id, &sp.host_id).map(|h| h.base.model_id.as_str())
            != Some(model_id)
        {
            continue;
        }
        let limit = if args.grid_host_ids.contains(&sp.host_id) {
            args.grid_host_fan_radius_mm
        } else {
            args.fan_radius_mm
        };
        let dist2 = ddx * ddx + ddy * ddy + ddz * ddz;
        let lateral_mm = lateral2.sqrt();
        if lateral_mm < nearest_host_mm {
            nearest_host_mm = lateral_mm;
        }
        if lateral2 > limit * limit {
            continue;
        }
        let in_reach = dist2 <= limit * limit;
        let rescue_span_mm = args
            .rescue_span_override_mm
            .unwrap_or(if max_angle_deg > 0.0 {
                limit / js_sin((max_angle_deg * std::f64::consts::PI) / 180.0)
            } else {
                f64::INFINITY
            });
        let in_rescue = dist2 <= rescue_span_mm * rescue_span_mm;
        if !in_reach && !in_rescue {
            continue;
        }
        // The leaf must RISE: the host sample must sit below the target tip.
        let v_dist = target.z - sp.pos.z;
        if v_dist < 0.4 {
            if refusal == FanLeafRefusal::NoHost {
                refusal = FanLeafRefusal::SameZ;
            }
            continue;
        }
        let angle_deg =
            (js_atan2((ddx * ddx + ddy * ddy).sqrt(), v_dist) * 180.0) / std::f64::consts::PI;
        if angle_deg > max_angle_deg {
            if refusal == FanLeafRefusal::NoHost {
                refusal = FanLeafRefusal::Angle;
            }
            continue;
        }
        if lateral_mm < nearest_steep_mm {
            nearest_steep_mm = lateral_mm;
            steep_angle_deg = angle_deg;
        }
        let c = FanCandidate {
            sp,
            dist2,
            angle_deg,
        };
        if in_reach {
            candidates.push(c);
        } else {
            rescue.push(c);
        }
    }
    let first_tier = !candidates.is_empty();
    let mut pool = if first_tier { candidates } else { rescue };
    let finite = |v: f64| v.is_finite().then_some(v);
    if pool.is_empty() {
        return FanLeafResult::Refused {
            reason: refusal,
            nearest_host_mm: finite(nearest_host_mm),
            nearest_steep_mm: finite(nearest_steep_mm),
            steep_angle_deg: finite(steep_angle_deg),
        };
    }
    let order = if first_tier {
        args.host_order
    } else {
        FanHostOrder::Nearest
    };
    // `a - b || c - d`: the second difference decides when the first is 0 or NaN.
    let or = |first: f64, second: f64| {
        if first != 0.0 && !first.is_nan() {
            first
        } else {
            second
        }
    };
    pool.sort_by(|a, b| {
        let d = match order {
            FanHostOrder::Nearest => or(a.dist2 - b.dist2, a.angle_deg - b.angle_deg),
            FanHostOrder::Steepest => or(a.angle_deg - b.angle_deg, a.dist2 - b.dist2),
        };
        d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut last_blocked: Option<FanLeafRefusal> = None;
    for FanCandidate {
        sp,
        dist2,
        angle_deg,
    } in pool
    {
        let parent_knot = knot_for(sp, free_knot_id(draft, args.knot_id_prefix));
        let (resolved_point, resolved_normal) = ctx.contact_at(target);
        let long_span = dist2.sqrt() > MAX_LEAF_SPAN_BEFORE_BRANCH_MM;
        if let Some(mesh) = mesh {
            let to_tip = Vec3::new(
                target.x - sp.pos.x,
                target.y - sp.pos.y,
                target.z - sp.pos.z,
            );
            let length = match js_hypot(&[to_tip.x, to_tip.y, to_tip.z]) {
                l if l == 0.0 || l.is_nan() => 1.0,
                l => l,
            };
            let inset = js_min(FAN_LINK_TIP_INSET_MM, length * 0.5);
            let short_of_tip = Vec3::new(
                target.x - (to_tip.x / length) * inset,
                target.y - (to_tip.y / length) * inset,
                target.z - (to_tip.z / length) * inset,
            );
            let blocked = if long_span {
                mesh.shaft_blocked(sp.pos, short_of_tip, 0.2)
            } else {
                contact_cone_collides(sp.pos, target, resolved_normal, None, mesh)
            };
            if blocked {
                last_blocked = Some(FanLeafRefusal::Blocked);
                continue;
            }
        }

        // Long spans route to branches with real shafts, for every origin.
        if long_span {
            let band = ctx.bands.active(ctx.live());
            let built = build_hosted_branch(
                &HostedBranchInput {
                    tip_pos: resolved_point,
                    tip_normal: resolved_normal,
                    model_id,
                    parent_knot: &parent_knot,
                    tip_contact_diameter_mm: Some(band.tip_contact_diameter_mm),
                    roots_diameter_mm: Some(band.root_diameter_mm),
                },
                band.shaft_diameter_mm,
                sp.diameter,
                ctx,
                &|attempt| {
                    attempt.support_data.error.is_some()
                        || mesh
                            .is_some_and(|m| branch_collides_with_sdf(&attempt.branch.segments, m))
                },
            );
            let Some(mut built) = built else {
                last_blocked = Some(FanLeafRefusal::Blocked);
                continue;
            };
            if branch_departure_angle_deg(&built.branch, parent_knot.pos) > max_angle_deg {
                last_blocked = Some(FanLeafRefusal::Angle);
                continue;
            }
            if args.max_attachments > 0.0
                && is_host_at_attachment_capacity(
                    sp.host_type_id,
                    &sp.host_id,
                    args.max_attachments,
                    draft,
                )
            {
                last_blocked = Some(FanLeafRefusal::Capacity);
                continue;
            }
            built.branch.base.origin = Some(if args.origin == Some(SupportOriginId::Overhang) {
                SupportOriginId::Overhang
            } else {
                SupportOriginId::Island
            });
            let kind = built
                .branch
                .base
                .type_id
                .expect("a built member carries its typeId");
            let entity_id = built.branch.base.id.clone();
            return FanLeafResult::Ok {
                kind,
                parent_knot: Box::new(parent_knot),
                member: Box::new(SupportEntityValue::Branch(built.branch)),
                host_type_id: sp.host_type_id,
                host_id: sp.host_id.clone(),
                entity_id,
                dist_mm: dist2.sqrt(),
                angle_deg,
            };
        }

        let built = {
            let field = mesh.map(|m| m.field());
            build_leaf_data(
                &LeafBuildInput {
                    tip_pos: resolved_point,
                    surface_normal: resolved_normal,
                    model_id,
                    parent_knot: &parent_knot,
                    host_diameter_mm: sp.diameter,
                    tip_contact_diameter_mm: Some(
                        ctx.bands.active(ctx.live()).tip_contact_diameter_mm,
                    ),
                    mesh: field.as_deref().map(|f| f as &dyn CollisionField),
                },
                ctx.live(),
                &mut || ctx.id(),
            )
        };
        if built.support_data.error.is_some() {
            last_blocked = Some(FanLeafRefusal::Build);
            continue;
        }
        let mut leaf = built.leaf;
        if leaf_path_crosses_supports(
            parent_knot.pos,
            leaf.contact_cone.pos,
            0.25,
            draft,
            Some(&sp.host_id),
        ) {
            last_blocked = Some(FanLeafRefusal::Cross);
            continue;
        }
        if args.max_attachments > 0.0
            && is_host_at_attachment_capacity(
                sp.host_type_id,
                &sp.host_id,
                args.max_attachments,
                draft,
            )
        {
            last_blocked = Some(FanLeafRefusal::Capacity);
            continue;
        }
        if let Some(origin) = args.origin {
            leaf.base.origin = Some(origin);
        }
        let kind = leaf
            .base
            .type_id
            .expect("a built member carries its typeId");
        let entity_id = leaf.base.id.clone();
        return FanLeafResult::Ok {
            kind,
            parent_knot: Box::new(parent_knot),
            member: Box::new(SupportEntityValue::Leaf(leaf)),
            host_type_id: sp.host_type_id,
            host_id: sp.host_id.clone(),
            entity_id,
            dist_mm: dist2.sqrt(),
            angle_deg,
        };
    }
    FanLeafResult::Refused {
        reason: last_blocked.unwrap_or(refusal),
        nearest_host_mm: None,
        nearest_steep_mm: None,
        steep_angle_deg: None,
    }
}

/// Consolidation fallback: attach the standalone trunk's tip to the steepest
/// eligible host sample as a ROUTED BRANCH. Returns the knot and the branch
/// for the caller to add to `pruned` ([`add_fanned`]), with the branch's id
/// and type.
#[allow(clippy::too_many_arguments)]
pub fn build_consolidation_branch(
    tip: Vec3,
    tip_normal: Vec3,
    model_id: &str,
    pool: &[FanShaftPoint],
    pruned: &SupportState,
    radius_mm: f64,
    max_attachments: f64,
    knot_id: &str,
    ctx: &PlanCtx<'_>,
) -> Option<(Knot, SupportEntityValue, String, SupportTypeId)> {
    let mesh = ctx.mesh.as_ref();
    let max_link = js_min(50.0, member_max_angle_from_vertical_deg(ctx.live()));
    let mut best: Option<&FanShaftPoint> = None;
    let mut best_angle_deg = f64::INFINITY;
    for sp in pool {
        if host_entity_of(pruned, sp.host_type_id, &sp.host_id).map(|h| h.base.model_id.as_str())
            != Some(model_id)
        {
            continue;
        }
        let ddx = sp.pos.x - tip.x;
        let ddy = sp.pos.y - tip.y;
        let ddz = sp.pos.z - tip.z;
        if ddx * ddx + ddy * ddy + ddz * ddz > radius_mm * radius_mm {
            continue;
        }
        let v_dist = tip.z - sp.pos.z;
        if v_dist < 1.5 {
            continue;
        }
        let angle_deg = (js_atan2(js_hypot(&[ddx, ddy]), v_dist) * 180.0) / std::f64::consts::PI;
        if angle_deg > max_link {
            continue;
        }
        if angle_deg < best_angle_deg {
            best_angle_deg = angle_deg;
            best = Some(sp);
        }
    }
    let best = best?;
    if max_attachments > 0.0
        && is_host_at_attachment_capacity(best.host_type_id, &best.host_id, max_attachments, pruned)
    {
        return None;
    }

    let parent_knot = knot_for(best, free_knot_id(pruned, knot_id));
    let band = ctx.bands.active(ctx.live());
    let mut built = build_hosted_branch(
        &HostedBranchInput {
            tip_pos: tip,
            tip_normal,
            model_id,
            parent_knot: &parent_knot,
            tip_contact_diameter_mm: Some(band.tip_contact_diameter_mm),
            roots_diameter_mm: Some(band.root_diameter_mm),
        },
        band.shaft_diameter_mm,
        best.diameter,
        ctx,
        &|attempt| {
            attempt.support_data.error.is_some()
                || mesh.is_some_and(|m| branch_collides_with_sdf(&attempt.branch.segments, m))
        },
    )?;
    if branch_departure_angle_deg(&built.branch, parent_knot.pos)
        > member_max_angle_from_vertical_deg(ctx.live())
    {
        return None;
    }
    let cone_tip = built.branch.contact_cone.as_ref().map_or(tip, |c| c.pos);
    if leaf_path_crosses_supports(parent_knot.pos, cone_tip, 0.25, pruned, Some(&best.host_id)) {
        return None;
    }

    built.branch.base.origin = Some(SupportOriginId::Overhang);
    let kind = built
        .branch
        .base
        .type_id
        .expect("a built member carries its typeId");
    let branch_id = built.branch.base.id.clone();
    Some((
        parent_knot,
        SupportEntityValue::Branch(built.branch),
        branch_id,
        kind,
    ))
}

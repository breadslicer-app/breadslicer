//! Hanging a tip from a support nearby: as a leaf when the span is short,
//! else as a branch with a shaft of its own; and the branch a lone pillar
//! becomes when it joins a neighbour.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::helpers::{
    FanShaftPoint, HostedBranchInput, PlanCtx, branch_collides_with_sdf,
    branch_departure_angle_deg, build_hosted_branch, contact_cone_collides, free_knot_id,
    host_entity_of, host_shaft, is_host_at_attachment_capacity, leaf_path_crosses_supports,
    member_max_angle_from_vertical_deg,
};
use crate::geom::math_utils::{js_atan2, js_hypot, js_min, js_sin};
use crate::supports::auto_support::constants::{
    FAN_LINK_TIP_INSET_MM, MAX_LEAF_SPAN_BEFORE_BRANCH_MM,
};
use crate::supports::auto_support::support_draft::{draft_add_entity, draft_add_primitive};
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::support_kinds::{
    PlacementPrimitive, SupportEntityValue, SupportOriginId, SupportTypeId,
};
use crate::supports::support_types::leaf::leaf_builder::{LeafBuildInput, build_leaf_data};
use crate::supports::types::{Knot, SupportState, Vec3};

/// Why a tip could not be hung.
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

/// Which shaft point a tip tries first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanHostOrder {
    Steepest,
    Nearest,
}

/// Where a tip was hung, or why not.
#[derive(Debug, Clone, PartialEq)]
pub enum FanLeafResult {
    /// The knot on the host and the member hung from it, for the caller to
    /// add ([`add_fanned`]).
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
        /// How far (seen from above) the nearest shaft point of the model was.
        nearest_host_mm: Option<f64>,
        /// The nearest that was steep enough, and its lean.
        nearest_steep_mm: Option<f64>,
        steep_angle_deg: Option<f64>,
    },
}

/// How a tip is hung.
#[derive(Debug)]
pub struct FanArgs<'a> {
    pub target: Vec3,
    pub model_id: &'a str,
    /// The shaft points a member may start from.
    pub shaft_points: &'a [FanShaftPoint],
    pub grid_host_ids: &'a HashSet<String>,
    pub knot_id_prefix: &'a str,
    /// How far (seen from above) from a shaft point the tip may be, mm; a
    /// grid pillar's reach is the second.
    pub fan_radius_mm: f64,
    pub grid_host_fan_radius_mm: f64,
    /// The most a member leans from vertical, degrees.
    pub max_angle_deg: f64,
    /// The most members a host carries; 0 for no cap.
    pub max_attachments: f64,
    pub draft: &'a SupportState,
    pub origin: Option<SupportOriginId>,
    pub host_order: FanHostOrder,
    /// The longest member of the wider second try, past its usual
    /// `reach / sin(max lean)`.
    pub rescue_span_override_mm: Option<f64>,
}

/// A shaft point a member may start from.
#[derive(Clone)]
struct Start<'p> {
    point: &'p FanShaftPoint,
    dist2: f64,
    lean_deg: f64,
}

fn knot_at(point: &FanShaftPoint, id: String) -> Knot {
    Knot {
        id,
        parent_shaft_id: point
            .segment_id
            .clone()
            .unwrap_or_else(|| point.host_id.clone()),
        t: point.t,
        pos: point.pos,
        diameter: Some(point.diameter + 0.125),
    }
}

/// Adds a hung member to `draft`: its knot, then the member.
pub fn add_fanned(
    draft: &mut SupportState,
    kind: SupportTypeId,
    parent_knot: Knot,
    member: SupportEntityValue,
) {
    draft_add_primitive(draft, PlacementPrimitive::Knot(parent_knot));
    draft_add_entity(draft, kind, member);
}

/// The tip of `args` hung from a shaft point of its model below it, within
/// reach and lean: those in reach in the host order, else (a wider try) the
/// nearest within the longest member the lean allows. Each start is tried in
/// turn until a member builds, clears the model and the supports, and its
/// host has room.
pub fn fan_leaf_to_host(args: &FanArgs<'_>, ctx: &PlanCtx<'_>) -> FanLeafResult {
    let (target, model_id, draft, max_lean) =
        (args.target, args.model_id, args.draft, args.max_angle_deg);
    let mesh = ctx.mesh.as_ref();
    let mut near: Vec<Start<'_>> = Vec::new();
    let mut wider: Vec<Start<'_>> = Vec::new();
    let mut refusal = FanLeafRefusal::NoHost;
    let mut nearest_host = f64::INFINITY;
    let mut nearest_steep = f64::INFINITY;
    let mut steep_lean = f64::INFINITY;
    let reach = args.fan_radius_mm.max(args.grid_host_fan_radius_mm);
    for point in args.shaft_points {
        let (dx, dy, dz) = (
            point.pos.x - target.x,
            point.pos.y - target.y,
            point.pos.z - target.z,
        );
        let across2 = dx * dx + dy * dy;
        // Out of every reach and no nearer than the nearest host: it can
        // change nothing (and the host lookup costs).
        if across2 > reach * reach && across2.sqrt() >= nearest_host {
            continue;
        }
        if host_shaft(draft, point.host_type_id, &point.host_id).map(|(b, _)| b.model_id.as_str())
            != Some(model_id)
        {
            continue;
        }
        let limit = if args.grid_host_ids.contains(&point.host_id) {
            args.grid_host_fan_radius_mm
        } else {
            args.fan_radius_mm
        };
        let dist2 = dx * dx + dy * dy + dz * dz;
        let across = across2.sqrt();
        if across < nearest_host {
            nearest_host = across;
        }
        if across2 > limit * limit {
            continue;
        }
        let in_reach = dist2 <= limit * limit;
        let longest = args.rescue_span_override_mm.unwrap_or(if max_lean > 0.0 {
            limit / js_sin((max_lean * std::f64::consts::PI) / 180.0)
        } else {
            f64::INFINITY
        });
        if !in_reach && dist2 > longest * longest {
            continue;
        }
        // A member rises from its host to the tip.
        let rise = target.z - point.pos.z;
        if rise < 0.4 {
            if refusal == FanLeafRefusal::NoHost {
                refusal = FanLeafRefusal::SameZ;
            }
            continue;
        }
        let lean = (js_atan2((dx * dx + dy * dy).sqrt(), rise) * 180.0) / std::f64::consts::PI;
        if lean > max_lean {
            if refusal == FanLeafRefusal::NoHost {
                refusal = FanLeafRefusal::Angle;
            }
            continue;
        }
        if across < nearest_steep {
            nearest_steep = across;
            steep_lean = lean;
        }
        let start = Start {
            point,
            dist2,
            lean_deg: lean,
        };
        if in_reach {
            near.push(start);
        } else {
            wider.push(start);
        }
    }
    let near_try = !near.is_empty();
    let mut starts = if near_try { near } else { wider };
    if starts.is_empty() {
        let finite = |v: f64| v.is_finite().then_some(v);
        return FanLeafResult::Refused {
            reason: refusal,
            nearest_host_mm: finite(nearest_host),
            nearest_steep_mm: finite(nearest_steep),
            steep_angle_deg: finite(steep_lean),
        };
    }
    let order = if near_try {
        args.host_order
    } else {
        FanHostOrder::Nearest
    };
    // The second difference breaks a tie (or a NaN) of the first.
    let then = |first: f64, second: f64| {
        if first != 0.0 && !first.is_nan() {
            first
        } else {
            second
        }
    };
    starts.sort_by(|a, b| {
        let d = match order {
            FanHostOrder::Nearest => then(a.dist2 - b.dist2, a.lean_deg - b.lean_deg),
            FanHostOrder::Steepest => then(a.lean_deg - b.lean_deg, a.dist2 - b.dist2),
        };
        d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut last = None;
    for Start {
        point,
        dist2,
        lean_deg,
    } in starts
    {
        let knot = knot_at(point, free_knot_id(draft, args.knot_id_prefix));
        let (contact, normal) = ctx.contact_at(target);
        let long = dist2.sqrt() > MAX_LEAF_SPAN_BEFORE_BRANCH_MM;
        if let Some(mesh) = mesh {
            // A long member's shaft (stopping short of the tip), or a short
            // one's cone, must clear the model.
            let way = Vec3::new(
                target.x - point.pos.x,
                target.y - point.pos.y,
                target.z - point.pos.z,
            );
            let length = match js_hypot(&[way.x, way.y, way.z]) {
                l if l == 0.0 || l.is_nan() => 1.0,
                l => l,
            };
            let short = js_min(FAN_LINK_TIP_INSET_MM, length * 0.5);
            let end = Vec3::new(
                target.x - (way.x / length) * short,
                target.y - (way.y / length) * short,
                target.z - (way.z / length) * short,
            );
            let blocked = if long {
                mesh.shaft_blocked(point.pos, end, 0.2)
            } else {
                contact_cone_collides(point.pos, target, normal, None, mesh)
            };
            if blocked {
                last = Some(FanLeafRefusal::Blocked);
                continue;
            }
        }
        let full = |draft: &SupportState| {
            args.max_attachments > 0.0
                && is_host_at_attachment_capacity(
                    point.host_type_id,
                    &point.host_id,
                    args.max_attachments,
                    draft,
                )
        };
        let hung = |kind, member, entity_id| FanLeafResult::Ok {
            kind,
            parent_knot: Box::new(knot.clone()),
            member: Box::new(member),
            host_type_id: point.host_type_id,
            host_id: point.host_id.clone(),
            entity_id,
            dist_mm: dist2.sqrt(),
            angle_deg: lean_deg,
        };
        if long {
            let band = ctx.bands.active(ctx.live());
            let Some(mut built) = build_hosted_branch(
                &HostedBranchInput {
                    tip_pos: contact,
                    tip_normal: normal,
                    model_id,
                    parent_knot: &knot,
                    tip_contact_diameter_mm: Some(band.tip_contact_diameter_mm),
                    roots_diameter_mm: Some(band.root_diameter_mm),
                },
                band.shaft_diameter_mm,
                point.diameter,
                ctx,
                &|attempt| {
                    attempt.support_data.error.is_some()
                        || mesh
                            .is_some_and(|m| branch_collides_with_sdf(&attempt.branch.segments, m))
                },
            ) else {
                last = Some(FanLeafRefusal::Blocked);
                continue;
            };
            if branch_departure_angle_deg(&built.branch, knot.pos) > max_lean {
                last = Some(FanLeafRefusal::Angle);
                continue;
            }
            if full(draft) {
                last = Some(FanLeafRefusal::Capacity);
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
                .expect("a built member has its type");
            let id = built.branch.base.id.clone();
            return hung(kind, SupportEntityValue::Branch(built.branch), id);
        }
        let built = {
            let field = mesh.map(|m| m.field());
            build_leaf_data(
                &LeafBuildInput {
                    tip_pos: contact,
                    surface_normal: normal,
                    model_id,
                    parent_knot: &knot,
                    host_diameter_mm: point.diameter,
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
            last = Some(FanLeafRefusal::Build);
            continue;
        }
        let mut leaf = built.leaf;
        if ctx.cone_crosses(&leaf.contact_cone) {
            last = Some(FanLeafRefusal::Blocked);
            continue;
        }
        if leaf_path_crosses_supports(
            knot.pos,
            leaf.contact_cone.pos,
            0.25,
            draft,
            Some(&point.host_id),
        ) {
            last = Some(FanLeafRefusal::Cross);
            continue;
        }
        if full(draft) {
            last = Some(FanLeafRefusal::Capacity);
            continue;
        }
        if let Some(origin) = args.origin {
            leaf.base.origin = Some(origin);
        }
        let kind = leaf.base.type_id.expect("a built member has its type");
        let id = leaf.base.id.clone();
        return hung(kind, SupportEntityValue::Leaf(leaf), id);
    }
    FanLeafResult::Refused {
        reason: last.unwrap_or(refusal),
        nearest_host_mm: None,
        nearest_steep_mm: None,
        steep_angle_deg: None,
    }
}

/// A lone pillar's tip joined to the steepest shaft point of its model
/// within `radius_mm` and at least 1.5 mm below, as a routed branch (when
/// its host has room and the branch clears the model and the supports):
/// its knot, the branch, its id and kind, for the caller to add to `pruned`.
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
    let mut best_lean = f64::INFINITY;
    for point in pool {
        if host_entity_of(pruned, point.host_type_id, &point.host_id)
            .map(|h| h.base.model_id.as_str())
            != Some(model_id)
        {
            continue;
        }
        let (dx, dy, dz) = (
            point.pos.x - tip.x,
            point.pos.y - tip.y,
            point.pos.z - tip.z,
        );
        if dx * dx + dy * dy + dz * dz > radius_mm * radius_mm {
            continue;
        }
        let rise = tip.z - point.pos.z;
        if rise < 1.5 {
            continue;
        }
        let lean = (js_atan2(js_hypot(&[dx, dy]), rise) * 180.0) / std::f64::consts::PI;
        if lean <= max_link && lean < best_lean {
            best_lean = lean;
            best = Some(point);
        }
    }
    let best = best?;
    if max_attachments > 0.0
        && is_host_at_attachment_capacity(best.host_type_id, &best.host_id, max_attachments, pruned)
    {
        return None;
    }
    let knot = knot_at(best, free_knot_id(pruned, knot_id));
    let band = ctx.bands.active(ctx.live());
    let mut built = build_hosted_branch(
        &HostedBranchInput {
            tip_pos: tip,
            tip_normal,
            model_id,
            parent_knot: &knot,
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
    if branch_departure_angle_deg(&built.branch, knot.pos)
        > member_max_angle_from_vertical_deg(ctx.live())
    {
        return None;
    }
    let cone_tip = built.branch.contact_cone.as_ref().map_or(tip, |c| c.pos);
    if leaf_path_crosses_supports(knot.pos, cone_tip, 0.25, pruned, Some(&best.host_id)) {
        return None;
    }
    built.branch.base.origin = Some(SupportOriginId::Overhang);
    let kind = built
        .branch
        .base
        .type_id
        .expect("a built member has its type");
    let id = built.branch.base.id.clone();
    Some((knot, SupportEntityValue::Branch(built.branch), id, kind))
}

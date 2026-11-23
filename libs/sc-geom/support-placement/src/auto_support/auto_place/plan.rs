//! One model's run: its contacts by the zone rules, each placed in turn;
//! lone trunks that can hang from a neighbour made members, long island
//! branches hung shorter; islands still bare hung from what stands near, and
//! wide flat ones given more contacts; then the final check (with what it
//! takes placed again), the tip cones sized to their hosts, and the braces.

use bread_support_geom::perf;
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::fan::{
    FanArgs, FanHostOrder, FanLeafRefusal, FanLeafResult, add_fanned, build_consolidation_branch,
    fan_leaf_to_host,
};
use super::helpers::{
    HostedBranchInput, MAX_FANNING_PASSES, OrphanInfo, OrphanReason, PlanCtx,
    branch_collides_with_sdf, build_hosted_branch, collect_contact_positions,
    collect_fan_shaft_points, collect_fan_shaft_points_near, collect_host_entities,
    count_attachments_on_host, filter_already_supported, host_segment_span,
    is_host_at_attachment_capacity, member_max_angle_from_vertical_deg, rehost_legacy_knots,
    resolve_surface_normal, sync_contact_cone_diameters_with, validate_and_cull_orphans,
};
use super::place::{PlaceArgs, grid_join_height, place_one_candidate};
use super::report::{CavityFallback, ForestReport, PlacementDiagnostics};
use crate::auto_support::coverage::{compute_region_coverage, coverage_radius_for_area};
use crate::auto_support::support_draft::{draft_add_entity, draft_add_primitive};
use crate::auto_support::types::{AutoSupportMesh, CandidatePoint, CandidateSource};
use crate::auto_support::zone_candidates::{KeepOut, ZoneInput, generate_zone_candidates};
use crate::volume_analysis::islands::types::{DetectedIsland, IslandSource};
use bread_support_geom::geom::math_utils::{js_max, js_min, js_round};
use bread_support_model::auto_bracing::mesh_clearance::BraceMeshes;
use bread_support_model::auto_support::constants::{
    CONSOLIDATION_BRANCH_MIN_HEIGHT_MM, CONSOLIDATION_FAN_RADIUS_MM, CONSOLIDATION_MAX_ANGLE_DEG,
    GRID_HOST_FAN_RADIUS_MM, MIN_LEAF_FAN_RADIUS_MM,
};
use bread_support_model::auto_support::settings::{
    AutoSupportSettings, normalize_auto_support_settings,
};
use bread_support_model::model_mesh::ModelMesh;
use bread_support_model::support_kinds::{
    PlacementPrimitive, SupportEntityValue, SupportOriginId, SupportTypeId, contact_bridge_types,
    get_support_type_descriptor, is_origin_convertible_to_tree,
};
use bread_support_model::types::{Knot, SupportState, Vec3};

/// What a run tells: islands it left bare, why contacts got nothing, how
/// placement went and what the final check took.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPlaceAnalytics {
    pub islands_covered: i64,
    pub islands_uncovered: i64,
    pub rejection_reasons: IndexMap<String, usize>,
    /// The share of the islands' area the tips reach.
    pub area_coverage: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementDiagnostics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forest_report: Option<ForestReport>,
}

/// What a run placed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPlaceResult {
    /// How many of each kind, in registry order.
    pub placed: IndexMap<SupportTypeId, i64>,
    pub rejected_candidates: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics: Option<AutoPlaceAnalytics>,
}

/// A run's supports and what it tells.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoSupportPlan {
    pub support: SupportState,
    pub analytics: AutoPlaceAnalytics,
    pub result: AutoPlaceResult,
}

/// What a run reads beyond the shared context.
pub struct PlanInput<'a> {
    pub islands: &'a [DetectedIsland],
    pub model_id: &'a str,
    pub settings: &'a AutoSupportSettings,
    /// The supports the run adds to (the other models' already in).
    pub state: &'a SupportState,
    /// Every model of the plate, which supports placed again stay clear of.
    pub brace_meshes: &'a BraceMeshes<'a>,
    /// Braces a run's supports (the draft, the settings in force, the
    /// supports already on the plate, a source of ids).
    pub brace: &'a Brace<'a>,
    pub zone: ZonePlanInput<'a>,
}

impl std::fmt::Debug for PlanInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanInput")
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

/// How a run's supports are braced.
pub type Brace<'a> = dyn Fn(
        &SupportState,
        &bread_support_model::settings::types::SupportSettings,
        &SupportState,
        &mut dyn FnMut() -> String,
    ) -> SupportState
    + 'a;

/// What the zone rules read beyond the islands (#360).
#[derive(Clone, Copy, Debug)]
pub struct ZonePlanInput<'a> {
    /// The down-facing regions up to the stabilisation angle.
    pub steep: &'a [DetectedIsland],
    pub layer_height_mm: f64,
    /// Where no tip may land (#465).
    pub keep_out: &'a [KeepOut],
    /// The faces painted as detail, by face of the tree (#186).
    pub detail: Option<&'a [bool]>,
}

/// A tip within this of an island's contact covers it (islands without
/// contact cells); the base of the reach for those with them.
pub const SUPPORT_COVERAGE_RADIUS_MM: f64 = 4.0;

/// The islands the tips cover (90 % of its contact region within reach), and
/// the area they reach of all of it.
struct Coverage {
    covered: HashSet<String>,
    covered_area: f64,
    total_area: f64,
}

impl Coverage {
    fn of(islands: &[DetectedIsland], tips: &[Vec3]) -> Self {
        let reach2 = SUPPORT_COVERAGE_RADIUS_MM * SUPPORT_COVERAGE_RADIUS_MM;
        let mut out = Coverage {
            covered: HashSet::new(),
            covered_area: 0.0,
            total_area: 0.0,
        };
        for island in islands {
            let area = island.area_mm2.unwrap_or(0.0);
            out.total_area += area;
            let share = if island.contact_voxels.as_ref().is_some_and(|v| v.count > 0) {
                compute_region_coverage(
                    island,
                    tips,
                    coverage_radius_for_area(area, SUPPORT_COVERAGE_RADIUS_MM),
                )
            } else if tips.iter().any(|t| {
                let (dx, dy, dz) = (
                    island.contact.x - t.x,
                    island.contact.y - t.y,
                    island.contact.z - t.z,
                );
                dx * dx + dy * dy + dz * dz <= reach2
            }) {
                1.0
            } else {
                0.0
            };
            out.covered_area += area * share;
            if share >= 0.9 {
                out.covered.insert(island.id.clone());
            }
        }
        out
    }

    fn area_share(&self) -> f64 {
        if self.total_area > 0.0 {
            self.covered_area / self.total_area
        } else {
            0.0
        }
    }
}

fn no_placements() -> IndexMap<SupportTypeId, i64> {
    SupportTypeId::ALL.into_iter().map(|t| (t, 0)).collect()
}

/// The contact to place again a support the final check took (#63, #236):
/// the one it was placed from, or one at its own tip for a support the run
/// did not place.
fn contact_again(
    draft: &SupportState,
    placed_from: &HashMap<String, CandidatePoint>,
    id: &str,
) -> Option<CandidatePoint> {
    if let Some(candidate) = placed_from.get(id) {
        return Some(candidate.clone());
    }
    let c = &draft.collections;
    let (base, cone) = c
        .leaves
        .get(id)
        .map(|l| (&l.base, Some(&l.contact_cone)))
        .or_else(|| {
            c.branches
                .get(id)
                .map(|b| (&b.base, b.contact_cone.as_ref()))
        })
        .or_else(|| c.trunks.get(id).map(|t| (&t.base, t.contact_cone.as_ref())))?;
    let cone = cone?;
    Some(CandidatePoint {
        id: id.to_owned(),
        tip_pos: cone.pos,
        tip_normal: cone.surface_normal.unwrap_or(cone.normal),
        model_id: base.model_id.clone(),
        source: if base.origin == Some(SupportOriginId::Overhang) {
            CandidateSource::Overhang
        } else {
            CandidateSource::Voxel
        },
        island_area_mm2: 0.05,
        z_height: cone.pos.z,
        priority: 0.0,
        tip_diameter_mm: None,
        grid_point: None,
        zone_size: None,
        enclosed: false,
    })
}

/// How far into a model a shaft placed again may go, past its own radius,
/// before it counts as through it (#236).
const CROSSING_SLACK_MM: f64 = 0.15;

/// Whether the trunk or branch `id` goes into any model of the plate on its
/// way up (#236): its axis every 0.25 mm against the models' faces, short of
/// its tip cone (2.5 mm), which is checked on its own.
fn crosses_a_model(draft: &SupportState, id: &str, meshes: &BraceMeshes<'_>) -> bool {
    let c = &draft.collections;
    let (spans, tip): (Vec<(Vec3, Vec3, f64)>, Option<Vec3>) = if let Some(trunk) = c.trunks.get(id)
    {
        (
            trunk
                .segments
                .iter()
                .filter_map(|seg| {
                    host_segment_span(draft, trunk, seg).map(|(a, b)| (a, b, seg.diameter / 2.0))
                })
                .collect(),
            trunk.contact_cone.as_ref().map(|cone| cone.pos),
        )
    } else if let Some(branch) = c.branches.get(id) {
        let mut from = c.knots.get(&branch.parent_knot_id).map(|k| k.pos);
        (
            branch
                .segments
                .iter()
                .filter_map(|seg| {
                    let start = seg.bottom_joint.as_ref().map(|j| j.pos).or(from)?;
                    let end = seg.top_joint.as_ref().map(|j| j.pos)?;
                    from = Some(end);
                    Some((start, end, seg.diameter / 2.0))
                })
                .collect(),
            branch.contact_cone.as_ref().map(|cone| cone.pos),
        )
    } else {
        return false;
    };
    meshes.values().any(|mesh| {
        let to_model = mesh.matrix.invert();
        spans.iter().any(|&(a, b, radius)| {
            let reach = radius - CROSSING_SLACK_MM;
            if reach <= 0.0 {
                return false;
            }
            let steps = (b.sub(a).length() / 0.25).ceil().max(1.0) as usize;
            (0..=steps).any(|k| {
                let p = a.add(b.sub(a).multiply_scalar(k as f64 / steps as f64));
                if tip.is_some_and(|t| t.distance_to(p) < 2.5) {
                    return false;
                }
                let mut touches = false;
                mesh.bvh
                    .faces_within(p.apply_matrix4(&to_model), reach, |_, _| touches = true);
                touches
            })
        })
    })
}

/// What a run keeps while it places.
struct Run {
    draft: SupportState,
    /// The supports raised for lattice points.
    grid_hosts: HashSet<String>,
    /// Those of them a lattice point of the run raised.
    lattice_raised: HashSet<String>,
    placed: IndexMap<SupportTypeId, i64>,
    rejected: usize,
    rejections: IndexMap<String, usize>,
    diagnostics: PlacementDiagnostics,
    /// The contact each support was placed from, by its id (#63, #236).
    placed_from: HashMap<String, CandidatePoint>,
}

impl Run {
    /// Places `candidate` in its zone; the id of what it placed.
    fn place(&mut self, candidate: &CandidatePoint, ctx: &PlanCtx<'_>) -> Option<String> {
        ctx.set_zone(candidate.zone_size);
        ctx.set_enclosed(candidate.enclosed);
        let out = place_one_candidate(
            candidate,
            &mut self.draft,
            &PlaceArgs {
                grid_host_ids: Some(&self.grid_hosts),
            },
            ctx,
            false,
        );
        ctx.set_zone(None);
        ctx.set_enclosed(false);
        let Some(kind) = out.kind else {
            self.rejected += 1;
            if let Some(reason) = out.rejected_reason {
                *self
                    .rejections
                    .entry(reason.as_str().to_owned())
                    .or_insert(0) += 1;
                let zone = candidate
                    .zone_size
                    .map_or("none".to_owned(), |z| z.to_string());
                *self
                    .diagnostics
                    .rejections_by_zone
                    .entry(format!("{zone}:{}", reason.as_str()))
                    .or_insert(0) += 1;
            }
            return None;
        };
        let id = out.entity_id?;
        self.placed_from.insert(id.clone(), candidate.clone());
        if candidate.grid_point == Some(true) && get_support_type_descriptor(kind).can_be_grid_host
        {
            self.grid_hosts.insert(id.clone());
            self.lattice_raised.insert(id.clone());
        }
        *self.placed.get_mut(&kind).expect("every type") += 1;
        if out.grid_fallback {
            self.diagnostics.grid_fallbacks += 1;
            if let Some(reason) = out.grid_fallback_reason {
                *self
                    .diagnostics
                    .grid_fallback_reasons
                    .entry(reason.to_owned())
                    .or_insert(0) += 1;
            }
        }
        if contact_bridge_types().contains(&kind) {
            self.diagnostics.cavity_fallbacks.push(CavityFallback {
                id: candidate.id.clone(),
                kind,
                tip: candidate.tip_pos,
                fan_refusal: out.cavity_fan_refusal,
            });
        }
        Some(id)
    }

    /// A trunk (`host_id`) became the member `member_id`, of `kind`.
    fn became_member(&mut self, host_id: &str, kind: SupportTypeId, member_id: &str) {
        self.grid_hosts.remove(host_id);
        *self
            .placed
            .get_mut(&SupportTypeId::Trunk)
            .expect("every type") -= 1;
        *self.placed.get_mut(&kind).expect("every type") += 1;
        if let Some(candidate) = self.placed_from.get(host_id).cloned() {
            self.placed_from.insert(member_id.to_owned(), candidate);
        }
    }
}

/// `tip` hung from one of `points` (see [`fan_leaf_to_host`]).
#[allow(clippy::too_many_arguments)]
fn hang_tip(
    run: &Run,
    tip: Vec3,
    model_id: &str,
    points: &[super::helpers::FanShaftPoint],
    grid_hosts: &HashSet<String>,
    prefix: String,
    reach: f64,
    max_lean: f64,
    cap: f64,
    origin: Option<SupportOriginId>,
    order: FanHostOrder,
    ctx: &PlanCtx<'_>,
) -> FanLeafResult {
    fan_leaf_to_host(
        &FanArgs {
            target: tip,
            model_id,
            shaft_points: points,
            grid_host_ids: grid_hosts,
            knot_id_prefix: &prefix,
            fan_radius_mm: reach,
            grid_host_fan_radius_mm: GRID_HOST_FAN_RADIUS_MM,
            max_angle_deg: max_lean,
            max_attachments: cap,
            draft: &run.draft,
            origin,
            host_order: order,
            rescue_span_override_mm: None,
        },
        ctx,
    )
}

/// A lone trunk of `model_id` that can hang from a neighbour becomes a
/// member, up to three rounds while any does: one on no lattice node (with
/// pillars, only nodes keep pillars of their own, #464), or one raised for a
/// lattice point, a lone overhang or a reinforcement. A leaf or branch by
/// the nearest host, else (high enough, the way blocked) a routed branch.
fn make_members(
    run: &mut Run,
    model_id: &str,
    auto: &AutoSupportSettings,
    reach: f64,
    max_lean: f64,
    ctx: &PlanCtx<'_>,
) {
    let live = ctx.live();
    let no_hosts = HashSet::new();
    for round in 0..3 {
        let mut made = 0;
        let mut points = collect_fan_shaft_points(&run.draft);
        let hosts: Vec<(SupportTypeId, String)> = collect_host_entities(&run.draft)
            .into_iter()
            .map(|h| (h.host_type_id, h.host_id.to_owned()))
            .collect();
        for (kind, host_id) in hosts {
            // The trunk as it stands now (an earlier one may have changed it).
            let Some(trunk) = run.draft.collections.trunks.get(&host_id).cloned() else {
                continue;
            };
            if !live.auto_support.share_hosts && trunk.base.model_id != model_id {
                continue;
            }
            let off_node = live.grid.enabled
                && live.grid.pillars
                && run
                    .draft
                    .collections
                    .roots
                    .get(&trunk.root_id)
                    .is_some_and(|r| {
                        let step = live.grid.spacing_mm;
                        let off = |v: f64| (v - (v / step).round() * step).abs() > 0.01;
                        off(r.transform.pos.x) || off(r.transform.pos.y)
                    });
            let origin = trunk.base.origin;
            let may_hang = off_node
                || (is_origin_convertible_to_tree(origin)
                    && (run.lattice_raised.contains(&host_id)
                        || origin == Some(SupportOriginId::Standalone)
                        || origin == Some(SupportOriginId::Reinforcement)));
            if !may_hang || count_attachments_on_host(kind, &host_id, &run.draft) > 0 {
                continue;
            }
            let Some(cone) = &trunk.contact_cone else {
                continue;
            };
            let (tip, tip_normal) = (cone.pos, cone.normal);
            // As a member it keeps its own zone size.
            let zone = ctx.zone_of_tip(cone.profile.contact_diameter_mm);
            let others: Vec<_> = points
                .iter()
                .filter(|p| p.host_id != host_id)
                .cloned()
                .collect();
            if others.is_empty() {
                break;
            }
            // Taken out while it tries, put back where it was if it stays.
            let taken_trunk = run.draft.collections.trunks.shift_remove_full(&host_id);
            let taken_root = run
                .draft
                .collections
                .roots
                .shift_remove_full(&trunk.root_id);
            ctx.set_zone(zone);
            let hung = hang_tip(
                run,
                tip,
                model_id,
                &others,
                &no_hosts,
                format!("auto-con-{host_id}-p{round}"),
                reach,
                max_lean,
                auto.max_attachments_per_trunk,
                Some(SupportOriginId::Overhang),
                FanHostOrder::Nearest,
                ctx,
            );
            let member = match hung {
                FanLeafResult::Ok {
                    kind,
                    parent_knot,
                    member,
                    entity_id,
                    ..
                } => Some((*parent_knot, *member, entity_id, kind)),
                FanLeafResult::Refused { reason, .. } => (tip.z
                    >= CONSOLIDATION_BRANCH_MIN_HEIGHT_MM
                    && matches!(
                        reason,
                        FanLeafRefusal::Blocked | FanLeafRefusal::Cross | FanLeafRefusal::SameZ
                    ))
                .then(|| {
                    build_consolidation_branch(
                        tip,
                        tip_normal,
                        model_id,
                        &others,
                        &run.draft,
                        reach,
                        auto.max_attachments_per_trunk,
                        &format!("auto-con-branch-{host_id}"),
                        ctx,
                    )
                })
                .flatten(),
            };
            ctx.set_zone(None);
            match member {
                Some((knot, member, member_id, kind)) => {
                    add_fanned(&mut run.draft, kind, knot, member);
                    points = others;
                    run.became_member(&host_id, kind, &member_id);
                    made += 1;
                }
                None => {
                    let c = &mut run.draft.collections;
                    if let Some((index, key, root)) = taken_root {
                        c.roots.shift_insert(index, key, root);
                    }
                    if let Some((index, key, trunk)) = taken_trunk {
                        c.trunks.shift_insert(index, key, trunk);
                    }
                }
            }
        }
        if made == 0 {
            break;
        }
    }
    ctx.set_zone(None);
}

/// How long an island branch has to be before it is hung from a neighbour
/// instead, mm (#464).
const REHANG_MIN_BRANCH_MM: f64 = 8.0;

/// Every island branch of `model_id` at least [`REHANG_MIN_BRANCH_MM`] long
/// with nothing on it is hung again from the nearest support within `reach`
/// (same contact, same tip size) when that is shorter; else it stays (#464).
/// Its knot leaves with it when nothing else hangs there.
fn rehang_island_branches(
    run: &mut Run,
    model_id: &str,
    reach: f64,
    max_lean: f64,
    cap: f64,
    ctx: &PlanCtx<'_>,
) {
    let ids: Vec<String> = run
        .draft
        .collections
        .branches
        .iter()
        .filter(|(_, b)| {
            b.base.model_id == model_id && b.base.origin == Some(SupportOriginId::Island)
        })
        .map(|(id, _)| id.clone())
        .collect();
    let no_hosts = HashSet::new();
    for id in ids {
        let Some(branch) = run.draft.collections.branches.get(&id).cloned() else {
            continue;
        };
        if count_attachments_on_host(SupportTypeId::Branch, &id, &run.draft) > 0 {
            continue;
        }
        let Some(cone) = &branch.contact_cone else {
            continue;
        };
        let Some(start) = run
            .draft
            .collections
            .knots
            .get(&branch.parent_knot_id)
            .map(|k| k.pos)
        else {
            continue;
        };
        let tip = cone.pos;
        let mut length = 0.0;
        let mut at = start;
        for joint in branch.segments.iter().filter_map(|s| s.top_joint.as_ref()) {
            length += joint.pos.sub(at).length();
            at = joint.pos;
        }
        length += tip.sub(at).length();
        if length < REHANG_MIN_BRANCH_MM {
            continue;
        }
        let zone = ctx.zone_of_tip(cone.profile.contact_diameter_mm);
        let taken = run.draft.collections.branches.shift_remove_full(&id);
        let points = collect_fan_shaft_points_near(&run.draft, tip, reach);
        ctx.set_zone(zone);
        let hung = hang_tip(
            run,
            tip,
            model_id,
            &points,
            &no_hosts,
            format!("auto-rehang-{id}"),
            reach,
            max_lean,
            cap,
            branch.base.origin,
            FanHostOrder::Nearest,
            ctx,
        );
        ctx.set_zone(None);
        match hung {
            FanLeafResult::Ok {
                kind,
                parent_knot,
                member,
                ..
            } if parent_knot.pos.sub(tip).length() < length => {
                add_fanned(&mut run.draft, kind, *parent_knot, *member);
                let c = &run.draft.collections;
                let used = c
                    .branches
                    .values()
                    .map(|b| &b.parent_knot_id)
                    .chain(c.leaves.values().map(|l| &l.parent_knot_id))
                    .chain(c.kickstands.values().map(|k| &k.host_knot_id))
                    .any(|k| *k == branch.parent_knot_id);
                if !used {
                    run.draft
                        .collections
                        .knots
                        .shift_remove(&branch.parent_knot_id);
                }
            }
            _ => {
                if let Some((index, key, branch)) = taken {
                    run.draft
                        .collections
                        .branches
                        .shift_insert(index, key, branch);
                }
            }
        }
    }
}

/// The islands still bare after placement hung from what stands near, a
/// few rounds while any takes; the number left bare.
fn hang_bare_islands(
    run: &mut Run,
    islands: &[DetectedIsland],
    model_id: &str,
    covered: &mut HashSet<String>,
    mut bare: i64,
    auto: &AutoSupportSettings,
    ctx: &PlanCtx<'_>,
) {
    let reach = js_max(MIN_LEAF_FAN_RADIUS_MM, auto.leaf_fan_radius_mm);
    let max_lean = js_min(
        auto.leaf_fan_max_angle_deg,
        member_max_angle_from_vertical_deg(ctx.live()),
    );
    let mut round = 0;
    while round < MAX_FANNING_PASSES && bare > 0 {
        let points = collect_fan_shaft_points(&run.draft);
        if points.is_empty() {
            break;
        }
        let mut hung_now = 0;
        for island in islands {
            if covered.contains(&island.id) {
                continue;
            }
            let origin = if island.source == IslandSource::Overhang {
                SupportOriginId::Overhang
            } else {
                SupportOriginId::Island
            };
            let FanLeafResult::Ok {
                kind,
                parent_knot,
                member,
                ..
            } = hang_tip(
                run,
                island.contact,
                model_id,
                &points,
                &run.grid_hosts,
                format!("auto-fan-{}-p{round}", island.id),
                reach,
                max_lean,
                auto.max_attachments_per_trunk,
                Some(origin),
                FanHostOrder::Steepest,
                ctx,
            )
            else {
                continue;
            };
            add_fanned(&mut run.draft, kind, *parent_knot, *member);
            hung_now += 1;
            *run.placed.get_mut(&kind).expect("every type") += 1;
            covered.insert(island.id.clone());
        }
        if hung_now == 0 {
            break;
        }
        bare -= hung_now;
        round += 1;
    }
}

/// A wide flat island (not an overhang) gets a contact every 2.5 mm over its
/// footprint, each a branch from the nearest top of a trunk within reach.
fn cover_flat_islands(
    run: &mut Run,
    islands: &[DetectedIsland],
    model_id: &str,
    auto: &AutoSupportSettings,
    ctx: &PlanCtx<'_>,
) {
    const LEAST_AREA_MM2: f64 = 1.5;
    const STEP_MM: f64 = 2.5;
    let live = ctx.live();
    let mesh = ctx.mesh.as_ref();
    let reach = js_max(MIN_LEAF_FAN_RADIUS_MM, auto.leaf_fan_radius_mm);
    struct Top {
        kind: SupportTypeId,
        id: String,
        pos: Vec3,
        diameter: f64,
    }
    let tops: Vec<Top> = collect_host_entities(&run.draft)
        .into_iter()
        .filter(|h| live.auto_support.share_hosts || h.entity.base.model_id == model_id)
        .filter_map(|h| {
            let last = h.entity.segments.last();
            let pos = last
                .and_then(|s| s.top_joint.as_ref())
                .map(|j| j.pos)
                .or_else(|| h.entity.contact_cone.as_ref().map(|c| c.pos))?;
            Some(Top {
                kind: h.host_type_id,
                id: h.host_id.to_owned(),
                pos,
                diameter: last.map_or(1.0, |s| s.diameter),
            })
        })
        .collect();
    for island in islands {
        if island.source == IslandSource::Overhang {
            continue;
        }
        let Some(cells) = island.contact_voxels.as_ref() else {
            continue;
        };
        if island.area_mm2.unwrap_or(0.0) < LEAST_AREA_MM2 || cells.count < 3 {
            continue;
        }
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for i in 0..cells.count {
            let (x, y) = (cells.x(i), cells.y(i));
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x > max_x {
                max_x = x;
            }
            if y > max_y {
                max_y = y;
            }
        }
        let (width, depth) = (max_x - min_x, max_y - min_y);
        if width < STEP_MM && depth < STEP_MM {
            continue;
        }
        let cols = js_max(2.0, js_round(width / STEP_MM)) as usize;
        let rows = js_max(2.0, js_round(depth / STEP_MM)) as usize;
        for r in 0..rows {
            for c in 0..cols {
                let gx = min_x + (width * (c as f64 + 0.5)) / cols as f64;
                let gy = min_y + (depth * (r as f64 + 0.5)) / rows as f64;
                // The height of the nearest cell within a step (the last of
                // equals).
                let mut z = None;
                let mut nearest = STEP_MM * STEP_MM;
                for i in 0..cells.count {
                    let (dx, dy) = (gx - cells.x(i), gy - cells.y(i));
                    let d2 = dx * dx + dy * dy;
                    if d2 <= nearest {
                        nearest = d2;
                        z = Some(cells.z(i).unwrap_or(island.contact.z));
                    }
                }
                let Some(z) = z else {
                    continue;
                };
                if (gx - island.contact.x).powi(2) + (gy - island.contact.y).powi(2) < 1.0 {
                    continue;
                }
                let mut top: Option<&Top> = None;
                let mut nearest = reach * reach;
                for t in &tops {
                    let (dx, dy) = (gx - t.pos.x, gy - t.pos.y);
                    let d2 = dx * dx + dy * dy;
                    if d2 <= nearest {
                        nearest = d2;
                        top = Some(t);
                    }
                }
                let Some(top) = top else {
                    continue;
                };
                if is_host_at_attachment_capacity(
                    top.kind,
                    &top.id,
                    auto.max_attachments_per_trunk,
                    &run.draft,
                ) {
                    continue;
                }
                let (point, normal) = resolve_surface_normal(Vec3::new(gx, gy, z), mesh);
                let knot = Knot {
                    id: format!("auto-overhang-{}-{}-{r}-{c}", top.id, island.id),
                    parent_shaft_id: top.id.clone(),
                    pos: top.pos,
                    diameter: Some(top.diameter + 0.1),
                    ..Knot::default()
                };
                let band = ctx.bands.active(live);
                let Some(built) = build_hosted_branch(
                    &HostedBranchInput {
                        tip_pos: point,
                        tip_normal: normal,
                        model_id,
                        parent_knot: &knot,
                        tip_contact_diameter_mm: Some(band.tip_contact_diameter_mm),
                        roots_diameter_mm: Some(band.root_diameter_mm),
                    },
                    band.shaft_diameter_mm,
                    top.diameter,
                    ctx,
                    &|attempt| {
                        attempt.support_data.error.is_some()
                            || mesh.is_some_and(|m| {
                                branch_collides_with_sdf(&attempt.branch.segments, m)
                            })
                    },
                ) else {
                    continue;
                };
                let mut branch = built.branch;
                branch.base.origin = Some(SupportOriginId::Island);
                let kind = branch.base.type_id.expect("a built member has its type");
                draft_add_primitive(&mut run.draft, PlacementPrimitive::Knot(knot));
                draft_add_entity(&mut run.draft, kind, SupportEntityValue::Branch(branch));
                *run.placed.get_mut(&kind).expect("every type") += 1;
            }
        }
    }
}

/// The final check: whatever it takes off its contact (a member of a host
/// it took, a member blocked on its way) placed again with the final sizes
/// (#63, #236), and checked once more with the supports placed again that
/// go through a model; what it took, reported.
fn check_and_place_again(
    run: &mut Run,
    meshes: &BraceMeshes<'_>,
    ctx: &PlanCtx<'_>,
) -> Vec<OrphanInfo> {
    let mesh = ctx.mesh.as_ref();
    let before = run.draft.clone();
    let (checked, mut taken) = validate_and_cull_orphans(&run.draft, mesh, &HashSet::new());
    if taken.is_empty() {
        return taken;
    }
    run.draft = checked;
    let again: Vec<(usize, CandidatePoint)> = taken
        .iter()
        .enumerate()
        .filter(|(_, o)| matches!(o.reason, OrphanReason::HostCulled | OrphanReason::Blocked))
        .filter_map(|(i, o)| contact_again(&before, &run.placed_from, &o.id).map(|c| (i, c)))
        .collect();
    if again.is_empty() {
        return taken;
    }
    let mut placed_again: Vec<(usize, String)> = Vec::new();
    for (i, candidate) in &again {
        let candidate = CandidatePoint {
            id: format!("{}-requeue", candidate.id),
            ..candidate.clone()
        };
        if let Some(id) = run.place(&candidate, ctx) {
            placed_again.push((*i, id));
        }
    }
    // Placed again, a support may take another route: measured exactly on
    // every model of the plate from its foot up.
    let crossing: HashSet<String> = placed_again
        .iter()
        .filter(|(_, id)| crosses_a_model(&run.draft, id, meshes))
        .map(|(_, id)| id.clone())
        .collect();
    let (rechecked, taken_again) = validate_and_cull_orphans(&run.draft, mesh, &crossing);
    let taken_again_ids: HashSet<String> = if rechecked != run.draft {
        run.draft = rechecked;
        taken_again.iter().map(|o| o.id.clone()).collect()
    } else {
        HashSet::new()
    };
    // One placed again and taken again stays reported once, as the one it
    // replaced, not placed again.
    let mut reported: HashSet<String> = taken.iter().map(|o| o.id.clone()).collect();
    for (i, id) in placed_again {
        if !taken_again_ids.contains(&id) {
            taken[i].replaced = true;
        }
        reported.insert(id);
    }
    taken.extend(
        taken_again
            .into_iter()
            .filter(|o| taken_again_ids.contains(&o.id) && !reported.contains(&o.id)),
    );
    taken
}

/// The run for one model; none when it has no contacts to place.
pub fn compute_auto_support_plan(
    input: &PlanInput<'_>,
    ctx: &PlanCtx<'_>,
) -> Option<AutoSupportPlan> {
    let auto = normalize_auto_support_settings(input.settings);
    let live = ctx.live();
    let (islands, model_id) = (input.islands, input.model_id);
    let mesh = ctx.mesh.as_ref();
    let nothing = |state: &SupportState| {
        Some(AutoSupportPlan {
            support: state.clone(),
            analytics: AutoPlaceAnalytics {
                islands_covered: 0,
                islands_uncovered: islands.len() as i64,
                rejection_reasons: IndexMap::new(),
                area_coverage: 0.0,
                placement: None,
                forest_report: None,
            },
            result: AutoPlaceResult {
                placed: no_placements(),
                rejected_candidates: 0,
                analytics: None,
            },
        })
    };

    perf::mark("auto:candidates");
    let mut candidates: Vec<CandidatePoint> = {
        let field = mesh.map(|m| m.field());
        let surface = mesh.zip(field.as_deref()).map(|(m, f)| AutoSupportMesh {
            mesh: ModelMesh {
                field: f,
                rays: m.rays(),
            },
            side: m.side,
            blocked: m.blocked,
        });
        generate_zone_candidates(&ZoneInput {
            islands,
            steep: input.zone.steep,
            rules: &auto.zone_rules,
            layer_height_mm: input.zone.layer_height_mm,
            mesh: surface,
            keep_out: input.zone.keep_out,
            detail: input.zone.detail,
        })
    };
    for c in &mut candidates {
        c.model_id = model_id.to_owned();
    }
    perf::measure("auto:candidates");
    if candidates.is_empty() {
        return nothing(input.state);
    }
    candidates = filter_already_supported(candidates, input.state);
    if candidates.is_empty() {
        return nothing(input.state);
    }

    perf::mark("auto:placement");
    let mut run = Run {
        draft: input.state.clone(),
        grid_hosts: HashSet::new(),
        lattice_raised: HashSet::new(),
        placed: no_placements(),
        rejected: 0,
        rejections: IndexMap::new(),
        diagnostics: PlacementDiagnostics::default(),
        placed_from: HashMap::new(),
    };
    // With pillars, the contacts go by how high they meet their node's
    // pillar, highest first: the lower ones hang from what it raised (#464).
    if live.grid.enabled && live.grid.pillars {
        let mut keyed: Vec<(f64, CandidatePoint)> = candidates
            .drain(..)
            .map(|c| {
                ctx.set_zone(c.zone_size);
                let join = grid_join_height(&c, ctx);
                ctx.set_zone(None);
                (join.unwrap_or(f64::NEG_INFINITY), c)
            })
            .collect();
        keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
        candidates = keyed.into_iter().map(|(_, c)| c).collect();
    }
    for candidate in &candidates {
        run.place(candidate, ctx);
    }
    perf::measure("auto:placement");

    perf::mark("auto:consolidation");
    let reach = js_max(auto.leaf_fan_radius_mm, CONSOLIDATION_FAN_RADIUS_MM);
    let max_lean = js_min(
        js_max(auto.leaf_fan_max_angle_deg, CONSOLIDATION_MAX_ANGLE_DEG),
        member_max_angle_from_vertical_deg(live),
    );
    make_members(&mut run, model_id, &auto, reach, max_lean, ctx);
    // Mini supports, unless the zone preset turns them off (#471).
    if !auto.zone_rules.keep_long_island_branches {
        rehang_island_branches(
            &mut run,
            model_id,
            reach,
            max_lean,
            auto.max_attachments_per_trunk,
            ctx,
        );
    }
    perf::measure("auto:consolidation");

    perf::mark("auto:fanning");
    let Coverage { mut covered, .. } =
        Coverage::of(islands, &collect_contact_positions(&run.draft));
    let bare = islands.len() as i64 - covered.len() as i64;
    // How placement went, before the islands are hung and the check runs.
    let placement = run.diagnostics.clone();
    hang_bare_islands(&mut run, islands, model_id, &mut covered, bare, &auto, ctx);
    perf::measure("auto:fanning");
    perf::mark("auto:surface-coverage");
    cover_flat_islands(&mut run, islands, model_id, &auto, ctx);
    perf::measure("auto:surface-coverage");

    let changed = run.placed.values().any(|&count| count > 0);
    let mut forest_report = None;
    if changed {
        perf::mark("auto:resize-report");
        run.draft = rehost_legacy_knots(&run.draft);
        let taken = check_and_place_again(&mut run, input.brace_meshes, ctx);
        run.draft = sync_contact_cone_diameters_with(&run.draft, auto.member_host_shaft_ratio);
        forest_report = Some(ForestReport {
            orphans: (!taken.is_empty()).then_some(taken),
        });
        perf::measure("auto:resize-report");
        if !auto.debug_skip_auto_bracing {
            perf::mark("auto:bracing");
            run.draft = (input.brace)(&run.draft, live, input.state, &mut || ctx.id());
            perf::measure("auto:bracing");
        }
    }

    // What the plan hands back covers (the check may have taken tips).
    let end = Coverage::of(islands, &collect_contact_positions(&run.draft));
    let analytics = AutoPlaceAnalytics {
        islands_covered: end.covered.len() as i64,
        islands_uncovered: islands.len() as i64 - end.covered.len() as i64,
        rejection_reasons: run.rejections,
        area_coverage: end.area_share(),
        placement: Some(placement),
        forest_report,
    };
    Some(AutoSupportPlan {
        support: run.draft,
        analytics: analytics.clone(),
        result: AutoPlaceResult {
            placed: run.placed,
            rejected_candidates: run.rejected,
            analytics: Some(analytics),
        },
    })
}

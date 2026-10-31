//! DragonFruit `src/supports/autoSupport/autoPlace.ts`,
//! `computeAutoSupportPlan`: the whole auto-support run against a local draft —
//! candidates, dedup, the already-supported filter, per-candidate placement,
//! overhang consolidation, coverage gap-fill, leaf fanning, flat-island
//! coverage branches, then the forest resize, orphan cull and report.
//!
//! What DragonFruit reads from its stores is passed in: the committed support
//! state (`getSnapshot()`, which the gap-fill reads), the raft mode
//! (`getRaftSettingsForModel`) and the live settings (`getSettings()`, on the
//! context), and the meshes auto-bracing registers for clearance. Logging,
//! timings and progress are DragonFruit's console and modal and are not
//! reproduced; nor is the store rollback, since nothing here commits.

use crate::perf;
use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::fan::{
    FanArgs, FanHostOrder, FanLeafResult, add_fanned, build_consolidation_branch, fan_leaf_to_host,
};
use super::helpers::{
    HostedBranchInput, MAX_FANNING_PASSES, OrphanInfo, OrphanReason, PlanCtx,
    branch_collides_with_sdf, build_hosted_branch, collect_contact_positions,
    collect_fan_shaft_points, collect_host_entities, compute_mesh_volume_mm3,
    count_attachments_on_host, filter_already_supported, is_host_at_attachment_capacity,
    member_max_angle_from_vertical_deg, rehost_legacy_knots, resolve_surface_normal,
    sync_contact_cone_diameters, validate_and_cull_orphans,
};
use super::place::{PlaceArgs, PlaceOutcome, RejectReason, place_one_candidate};
use super::report::{
    BySource, CavityFallback, ForestLedgerEntry, ForestReport, ForestScanMetrics,
    PlacementDiagnostics, ReportDiagnostics, build_forest_report, bump, fan_key,
};
use crate::supports::auto_bracing::auto_brace::build_auto_braced_snapshot;
use crate::supports::auto_bracing::mesh_clearance::BraceMeshes;
use crate::supports::auto_support::candidate_generation::{
    deduplicate_candidates, generate_candidates,
};
use crate::supports::auto_support::constants::{
    CONSOLIDATION_BRANCH_MIN_HEIGHT_MM, CONSOLIDATION_FAN_RADIUS_MM, CONSOLIDATION_MAX_ANGLE_DEG,
    GRID_HOST_FAN_RADIUS_MM, MIN_LEAF_FAN_RADIUS_MM,
};
use crate::supports::auto_support::coverage::{
    MAX_GAP_FILL_PASSES, build_gap_fill_candidates, collect_support_tips, compute_region_coverage,
    coverage_radius_for_area,
};
use crate::supports::auto_support::grid_placement::{
    generate_grid_candidates, should_use_density_grid,
};
use crate::supports::auto_support::load_budget::{
    LoadBudgetInput, PlacedContact, compute_load_budget,
};
use crate::supports::auto_support::minima_reinforcement::compute_minima_reinforcement_points;
use crate::supports::auto_support::parameter_sizing::{
    ModelSizingContext, RESIN_DENSITY_G_PER_MM3, model_sizing_factors, preset_for_area,
    size_parameters,
};
use crate::supports::auto_support::pose_stability::{
    PoseStability, is_slender_part, measure_pose_stability, needs_topple_coverage, posed_positions,
    steep_flat_needs_coverage,
};
use crate::supports::auto_support::settings::{
    AutoSupportSettings, SizingPreset, create_default_auto_support_settings,
    normalize_auto_support_settings,
};
use crate::supports::auto_support::stabilization::compute_stabilization_anchors;
use crate::supports::auto_support::support_draft::{draft_add_entity, draft_add_primitive};
use crate::supports::auto_support::types::{
    AutoSupportMesh, CandidatePoint, CandidateSource, js_compare,
};
use crate::supports::model_mesh::ModelMesh;
use crate::supports::support_type_registry::{
    PlacementPrimitive, SupportEntityValue, SupportOriginId, SupportPlacementMetric, SupportTypeId,
    contact_bridge_types, get_support_type_descriptor, is_auto_placed_type,
    is_origin_convertible_to_tree,
};
use crate::supports::support_types::trunk::trunk_replacement::max_connected_diameter::compute_forest_diameter_profile;
use crate::supports::types::{Knot, SupportState, Vec3};
use crate::three::Box3;
use crate::three::math_utils::{js_max, js_min, js_round};
use crate::utils::math::quantize_to_scale;
use crate::volume_analysis::islands::types::{DetectedIsland, IslandSource};

/// What a run did, as a code the UI resolves into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoPlaceStatus {
    Placed,
    NoCandidates,
    AllDeduplicated,
    AlreadySupported,
    Disabled,
}

/// Per-tier placement counts.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PresetCounts {
    pub detail: usize,
    pub structure: usize,
    pub anchor: usize,
}

impl PresetCounts {
    fn bump(&mut self, preset: SizingPreset) {
        match preset {
            SizingPreset::Detail => self.detail += 1,
            SizingPreset::Structure => self.structure += 1,
            SizingPreset::Anchor => self.anchor += 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SizeRange {
    pub min: f64,
    pub max: f64,
    pub avg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizingDebugInfo {
    pub model_volume_mm3: f64,
    pub estimated_weight_g: f64,
    pub total_candidates: f64,
    pub weight_per_support_g: f64,
    pub model_size_mm: f64,
    pub load_share_g: f64,
    pub size_factor: f64,
    pub load_factor: f64,
    pub avg_island_area_mm2: f64,
    pub standalone_hosts: i64,
    pub grid_infill_hosts: i64,
    pub shaft_diameter_range: SizeRange,
    pub tip_contact_range: SizeRange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPlaceAnalytics {
    pub islands_covered: i64,
    pub islands_uncovered: i64,
    pub presets: PresetCounts,
    pub rejection_reasons: IndexMap<String, usize>,
    pub area_coverage: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementDiagnostics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizing_debug: Option<SizingDebugInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forest_report: Option<ForestReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPlaceResult {
    /// One count per support type, in registry order.
    pub placed: IndexMap<SupportTypeId, i64>,
    pub rejected_candidates: usize,
    pub changed: bool,
    pub status: AutoPlaceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics: Option<AutoPlaceAnalytics>,
}

/// A fully computed run, ready to commit.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoSupportPlan {
    pub before: SupportState,
    pub support: SupportState,
    pub analytics: AutoPlaceAnalytics,
    pub result: AutoPlaceResult,
}

/// The run's inputs beyond the shared context.
pub struct PlanInput<'a> {
    pub islands: &'a [DetectedIsland],
    pub model_id: &'a str,
    /// `settingsOverride`; `None` normalizes the defaults.
    pub settings_override: Option<&'a AutoSupportSettings>,
    /// The state the run builds on (`baseState`).
    pub base_state: &'a SupportState,
    /// The committed store state (`getSnapshot()`), which the gap-fill reads.
    pub committed: &'a SupportState,
    /// `getRaftSettingsForModel(modelId).bottomMode !== 'off'`.
    pub has_raft: bool,
    /// The models registered for auto-bracing's clearance checks
    /// (`getAllMeshEntriesForAutoBrace()`).
    pub brace_meshes: &'a BraceMeshes<'a>,
}

/// A tip within this of an island's contact covers it (islands without
/// contact voxels); the base of the radius for those with them.
pub const SUPPORT_COVERAGE_RADIUS_MM: f64 = 4.0;

/// Which islands the tips cover: an island is covered when at least 90 % of
/// its contact region lies within reach of a tip.
struct IslandCoverage {
    supported_ids: HashSet<String>,
    covered_area: f64,
    total_area: f64,
}

impl IslandCoverage {
    fn area_fraction(&self) -> f64 {
        if self.total_area > 0.0 {
            self.covered_area / self.total_area
        } else {
            0.0
        }
    }
}

fn island_coverage(islands: &[DetectedIsland], tips: &[Vec3]) -> IslandCoverage {
    let cov_r2 = SUPPORT_COVERAGE_RADIUS_MM * SUPPORT_COVERAGE_RADIUS_MM;
    let mut out = IslandCoverage {
        supported_ids: HashSet::new(),
        covered_area: 0.0,
        total_area: 0.0,
    };
    for island in islands {
        let area = island.area_mm2.unwrap_or(0.0);
        out.total_area += area;
        let fraction = if island.contact_voxels.as_ref().is_some_and(|v| v.count > 0) {
            compute_region_coverage(
                island,
                tips,
                coverage_radius_for_area(area, SUPPORT_COVERAGE_RADIUS_MM),
            )
        } else {
            let c = island.contact;
            let hit = tips.iter().any(|t| {
                let (dx, dy, dz) = (c.x - t.x, c.y - t.y, c.z - t.z);
                dx * dx + dy * dy + dz * dz <= cov_r2
            });
            if hit { 1.0 } else { 0.0 }
        };
        out.covered_area += area * fraction;
        if fraction >= 0.9 {
            out.supported_ids.insert(island.id.clone());
        }
    }
    out
}

fn empty_placed_counts() -> IndexMap<SupportTypeId, i64> {
    SupportTypeId::ALL.into_iter().map(|t| (t, 0)).collect()
}

fn round2_mm(v: f64) -> f64 {
    quantize_to_scale(v, 100.0)
}

fn bump_reason(map: &mut IndexMap<String, usize>, reason: RejectReason) {
    *map.entry(reason.as_str().to_owned()).or_insert(0) += 1;
}

fn bump_source(by: &mut super::report::CandidatesBySource, source: CandidateSource) {
    match source {
        CandidateSource::Voxel => by.voxel += 1,
        CandidateSource::Minima => by.minima += 1,
        CandidateSource::Intersection => by.intersection += 1,
        CandidateSource::Overhang => by.overhang += 1,
        CandidateSource::Stabilization => by.stabilization += 1,
        CandidateSource::Reinforcement => by.reinforcement += 1,
    }
}

/// The run's mutable bookkeeping, shared by the main pass, the gap-fill and
/// the orphan re-place.
struct RunState {
    draft: SupportState,
    grid_host_ids: HashSet<String>,
    placed: IndexMap<SupportTypeId, i64>,
    rejected_count: usize,
    diagnostics: PlacementDiagnostics,
    /// Grid-infill hosts (`hostOriginById`; DragonFruit never records a
    /// coverage-fill one, so the map reduces to a set).
    grid_infill_ids: HashSet<String>,
    forest_ledger: Vec<ForestLedgerEntry>,
    placed_contacts: Vec<PlacedContact>,
    presets: PresetCounts,
    rejection_reasons: IndexMap<String, usize>,
}

impl RunState {
    fn place(
        &mut self,
        candidate: &CandidatePoint,
        sizing_ctx: Option<&ModelSizingContext>,
        ctx: &PlanCtx<'_>,
    ) -> Option<SupportTypeId> {
        let result: PlaceOutcome = place_one_candidate(
            candidate,
            &mut self.draft,
            &PlaceArgs {
                grid_host_ids: Some(&self.grid_host_ids),
                sizing_ctx,
            },
            ctx,
            false,
        );
        let placed_a_grid_host = result
            .kind
            .is_some_and(|k| get_support_type_descriptor(k).can_be_grid_host);
        if candidate.grid_point == Some(true)
            && placed_a_grid_host
            && let Some(id) = &result.entity_id
        {
            self.grid_host_ids.insert(id.clone());
        }
        match result.kind {
            None => {
                self.rejected_count += 1;
                if let Some(reason) = result.rejected_reason {
                    bump_reason(&mut self.rejection_reasons, reason);
                }
            }
            Some(kind) => {
                *self.placed.get_mut(&kind).expect("every type") += 1;
                if result.grid_fallback {
                    self.diagnostics.grid_fallbacks += 1;
                }
                if contact_bridge_types().contains(&kind) {
                    self.diagnostics.cavity_fallbacks.push(CavityFallback {
                        id: candidate.id.clone(),
                        kind,
                        tip: candidate.tip_pos,
                        fan_refusal: result.cavity_fan_refusal.clone(),
                    });
                }
            }
        }
        if let Some(preset) = result.preset {
            self.presets.bump(preset);
        }
        if result.kind.is_some() {
            self.placed_contacts.push(PlacedContact {
                tip: candidate.tip_pos,
                preset: result
                    .preset
                    .unwrap_or_else(|| preset_for_area(candidate.island_area_mm2)),
            });
        }

        bump_source(&mut self.diagnostics.candidates_by_source, candidate.source);
        if placed_a_grid_host && let Some(id) = &result.entity_id {
            if candidate.grid_point == Some(true) {
                self.diagnostics.hosts_by_kind.grid_infill += 1;
                self.grid_infill_ids.insert(id.clone());
            } else {
                self.diagnostics.hosts_by_kind.standalone += 1;
            }
        }
        if let Some(refusal) = result.fan_refusal {
            bump(&mut self.diagnostics.fan_refusals, fan_key(refusal));
        }
        if let Some(refusal) = result.merge_refusal {
            bump(&mut self.diagnostics.merge_refusals, refusal);
        }
        if let (Some(kind), Some(entity_id)) = (result.kind, &result.entity_id)
            && is_auto_placed_type(Some(kind))
        {
            self.forest_ledger.push(ForestLedgerEntry {
                display_id: candidate.id.clone(),
                kind,
                entity_id: entity_id.clone(),
                area_mm2: candidate.island_area_mm2,
                z_height: candidate.z_height,
                preset: result
                    .preset
                    .unwrap_or_else(|| preset_for_area(candidate.island_area_mm2)),
                band_shaft_mm: ctx.bands.active(ctx.live).shaft_diameter_mm,
            });
        }
        result.kind
    }

    /// A consolidated host yielded to `kind` (`entity_id`): move the tallies.
    fn record_conversion(
        &mut self,
        host_id: &str,
        host_type_id: SupportTypeId,
        kind: SupportTypeId,
        entity_id: &str,
    ) {
        self.grid_host_ids.remove(host_id);
        let kinds = &mut self.diagnostics.hosts_by_kind;
        if self.grid_infill_ids.contains(host_id) {
            kinds.grid_infill -= 1;
        } else {
            kinds.standalone -= 1;
        }
        *self.placed.get_mut(&host_type_id).expect("every type") -= 1;
        *self.placed.get_mut(&kind).expect("every type") += 1;
        if let Some(entry) = self.forest_ledger.iter().find(|e| e.entity_id == host_id) {
            let entry = ForestLedgerEntry {
                kind,
                entity_id: entity_id.to_owned(),
                ..entry.clone()
            };
            self.forest_ledger.push(entry);
        }
    }
}

/// `computeAutoSupportPlan`. `None` when auto-support is disabled.
pub fn compute_auto_support_plan(
    input: &PlanInput<'_>,
    ctx: &PlanCtx<'_>,
) -> Option<AutoSupportPlan> {
    let defaults = create_default_auto_support_settings();
    let auto = normalize_auto_support_settings(input.settings_override.unwrap_or(&defaults));
    if !auto.enabled {
        return None;
    }
    let live = ctx.live;
    let islands = input.islands;
    let model_id = input.model_id;
    let before = input.base_state.clone();

    let noop_plan = |draft: SupportState, status: AutoPlaceStatus| {
        let analytics = AutoPlaceAnalytics {
            islands_covered: 0,
            islands_uncovered: islands.len() as i64,
            presets: PresetCounts::default(),
            rejection_reasons: IndexMap::new(),
            area_coverage: 0.0,
            placement: None,
            sizing_debug: None,
            forest_report: None,
        };
        Some(AutoSupportPlan {
            before: before.clone(),
            support: draft,
            analytics,
            result: AutoPlaceResult {
                placed: empty_placed_counts(),
                rejected_candidates: 0,
                changed: false,
                status,
                analytics: None,
            },
        })
    };

    let mesh = ctx.mesh.as_ref();

    perf::mark("auto:candidates");
    // ── 1. Candidates ────────────────────────────────────────────────
    let pose_stability: Option<PoseStability> = mesh.map(|m| {
        measure_pose_stability(
            &posed_positions(m.bvh.positions(), &m.matrix),
            Some(m.bvh.index()),
            0.0,
            0.0,
            None,
            input.has_raft,
        )
    });
    let topple_coverage_needed = pose_stability.as_ref().is_none_or(needs_topple_coverage);
    let islands_to_cover: Vec<DetectedIsland> = islands
        .iter()
        .filter(|i| {
            i.steep_flat != Some(true)
                || steep_flat_needs_coverage(i.surface_area_mm2, topple_coverage_needed)
        })
        .cloned()
        .collect();
    let slender_part = pose_stability
        .as_ref()
        .is_some_and(|p| is_slender_part(p.height_mm, p.volume_mm3, p.bearing_area_mm2));

    let mut candidates: Vec<CandidatePoint> = {
        let field = mesh.map(|m| m.field());
        let auto_mesh = mesh.zip(field.as_deref()).map(|(m, f)| AutoSupportMesh {
            mesh: ModelMesh {
                field: f,
                rays: m.rays(),
            },
            side: m.side,
            blocked: m.blocked,
        });
        generate_candidates(&islands_to_cover, &auto, auto_mesh, live, ctx.bands)
    };
    for c in &mut candidates {
        c.model_id = model_id.to_owned();
    }

    // Stabilization anchors: standalone trunks that broaden a point/edge bearing.
    if auto.stabilization_enabled
        && let Some(m) = mesh
    {
        let anchors = compute_stabilization_anchors(
            m.bvh.positions(),
            Some(m.bvh.index()),
            &m.matrix,
            input.has_raft,
        );
        for (i, a) in anchors.iter().enumerate() {
            candidates.push(CandidatePoint {
                id: format!("stab-{i}"),
                tip_pos: Vec3::new(a.x, a.y, a.z),
                tip_normal: Vec3::new(0.0, 0.0, -1.0),
                model_id: model_id.to_owned(),
                source: CandidateSource::Stabilization,
                island_area_mm2: 0.05,
                z_height: a.z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: None,
            });
        }
    }

    // Minima reinforcement: a ring of contacts on a minima's own flank.
    if auto.minima_reinforcement_enabled
        && let Some(m) = mesh
    {
        let points =
            compute_minima_reinforcement_points(&islands_to_cover, m.rays(), m.side, &auto);
        for (i, p) in points.iter().enumerate() {
            candidates.push(CandidatePoint {
                id: format!("reinf-{}-{i}", p.island_id),
                tip_pos: Vec3::new(p.x, p.y, p.z),
                tip_normal: Vec3::new(0.0, 0.0, -1.0),
                model_id: model_id.to_owned(),
                source: CandidateSource::Reinforcement,
                island_area_mm2: 0.05,
                z_height: p.z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: None,
            });
        }
    }

    // The density grid replaces the single candidate of every region it serves.
    let overhang_islands: Vec<DetectedIsland> = islands_to_cover
        .iter()
        .filter(|i| i.source == IslandSource::Overhang)
        .cloned()
        .collect();
    let eligible: Vec<DetectedIsland> = overhang_islands
        .iter()
        .filter(|i| should_use_density_grid(i, &auto))
        .cloned()
        .collect();
    if !eligible.is_empty() {
        let mut generated = {
            let field = mesh.map(|m| m.field());
            let auto_mesh = mesh.zip(field.as_deref()).map(|(m, f)| AutoSupportMesh {
                mesh: ModelMesh {
                    field: f,
                    rays: m.rays(),
                },
                side: m.side,
                blocked: m.blocked,
            });
            generate_grid_candidates(
                &eligible,
                &auto,
                auto_mesh,
                slender_part,
                pose_stability.as_ref().map_or(0.0, |p| p.thickness_mm),
                live,
                ctx.bands,
            )
        };
        for c in &mut generated {
            c.model_id = model_id.to_owned();
        }
        let generated_region_ids: HashSet<&str> = eligible.iter().map(|i| i.id.as_str()).collect();
        if !generated.is_empty() {
            generated.extend(
                candidates
                    .into_iter()
                    .filter(|c| !generated_region_ids.contains(c.id.as_str())),
            );
            candidates = generated;
        }
    }
    if candidates.is_empty() {
        return noop_plan(before.clone(), AutoPlaceStatus::NoCandidates);
    }

    perf::measure("auto:candidates");
    perf::mark("auto:dedup-filter");
    // ── 2. Dedup, then the already-supported filter ─────────────────
    let before_dedup = candidates.len();
    candidates = deduplicate_candidates(&candidates, &auto);
    let deduped_candidates = candidates.len();
    if candidates.is_empty() {
        return noop_plan(before.clone(), AutoPlaceStatus::AllDeduplicated);
    }
    let before_support_filter = candidates.len();
    candidates = filter_already_supported(candidates, &before);
    let filtered_candidates = candidates.len();
    if candidates.is_empty() {
        return noop_plan(before.clone(), AutoPlaceStatus::AlreadySupported);
    }

    perf::measure("auto:dedup-filter");
    perf::mark("auto:placement");
    // ── 3. Placement ─────────────────────────────────────────────────
    let model_ctx: Option<ModelSizingContext> = mesh.map(|m| {
        let bbox = Box3::set_from_points(
            m.bvh
                .positions()
                .iter()
                .map(|p| Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))),
        )
        .apply_matrix4(&m.matrix);
        ModelSizingContext {
            model_volume_mm3: compute_mesh_volume_mm3(m.bvh),
            model_z_max_mm: Some(bbox.max.z),
            total_candidates: candidates.len() as f64,
            model_size_mm: Some(bbox.max.sub(bbox.min).length()),
        }
    });

    let mut run = RunState {
        draft: before.clone(),
        grid_host_ids: HashSet::new(),
        placed: empty_placed_counts(),
        rejected_count: 0,
        diagnostics: PlacementDiagnostics::default(),
        grid_infill_ids: HashSet::new(),
        forest_ledger: Vec::new(),
        placed_contacts: Vec::new(),
        presets: PresetCounts::default(),
        rejection_reasons: IndexMap::new(),
    };
    let placed_host_count = |placed: &IndexMap<SupportTypeId, i64>| -> i64 {
        SupportTypeId::ALL
            .into_iter()
            .filter(|t| get_support_type_descriptor(*t).can_be_grid_host)
            .map(|t| placed[&t])
            .sum()
    };

    for candidate in &candidates {
        run.place(candidate, model_ctx.as_ref(), ctx);
    }

    perf::measure("auto:placement");
    perf::mark("auto:consolidation");
    // ── Overhang→tree consolidation ─────────────────────────────────
    let con_fan_radius_mm = js_max(auto.leaf_fan_radius_mm, CONSOLIDATION_FAN_RADIUS_MM);
    let con_fan_max_angle_deg = js_min(
        js_max(auto.leaf_fan_max_angle_deg, CONSOLIDATION_MAX_ANGLE_DEG),
        member_max_angle_from_vertical_deg(live),
    );
    let mut con_refusals: IndexMap<String, usize> = IndexMap::new();
    let empty_hosts: HashSet<String> = HashSet::new();
    for pass in 0..3 {
        let mut converted_this_pass = 0;
        let mut pool = collect_fan_shaft_points(&run.draft);
        let hosts: Vec<(SupportTypeId, String)> = collect_host_entities(&run.draft)
            .into_iter()
            .map(|h| (h.host_type_id, h.host_id.to_owned()))
            .collect();
        for (host_type_id, host_id) in hosts {
            // The entity as it stands now: a conversion earlier in this loop
            // replaced the draft, and DragonFruit iterates the list it took.
            let Some(entity) = run.draft.collections.trunks.get(&host_id).cloned() else {
                continue;
            };
            if entity.base.model_id != model_id {
                continue;
            }
            let grid_infill = run.grid_infill_ids.contains(&host_id);
            let is_convertible = is_origin_convertible_to_tree(entity.base.origin)
                && (grid_infill
                    || entity.base.origin == Some(SupportOriginId::Standalone)
                    || entity.base.origin == Some(SupportOriginId::Reinforcement));
            if !is_convertible {
                continue;
            }
            if count_attachments_on_host(host_type_id, &host_id, &run.draft) > 0 {
                continue;
            }
            let Some(cone) = &entity.contact_cone else {
                continue;
            };
            let tip = cone.pos;
            let tip_normal = cone.normal;
            let host_pool: Vec<_> = pool
                .iter()
                .filter(|sp| sp.host_id != host_id)
                .cloned()
                .collect();
            if host_pool.is_empty() {
                break;
            }
            // The draft without this host. SoulCrafted: taken out of the draft
            // itself and put back where it was if the host stays, instead of
            // copying the whole draft per host; the state is the same either way.
            let pruned_trunk = run.draft.collections.trunks.shift_remove_full(&host_id);
            let pruned_root = run
                .draft
                .collections
                .roots
                .shift_remove_full(&entity.root_id);
            let fan = fan_leaf_to_host(
                &FanArgs {
                    target: tip,
                    model_id,
                    shaft_points: &host_pool,
                    grid_host_ids: &empty_hosts,
                    knot_id_prefix: &format!("auto-con-{host_id}-p{pass}"),
                    fan_radius_mm: con_fan_radius_mm,
                    grid_host_fan_radius_mm: GRID_HOST_FAN_RADIUS_MM,
                    max_angle_deg: con_fan_max_angle_deg,
                    max_attachments: auto.max_attachments_per_trunk,
                    draft: &run.draft,
                    origin: Some(SupportOriginId::Overhang),
                    host_order: FanHostOrder::Nearest,
                    rescue_span_override_mm: None,
                },
                ctx,
            );
            match fan {
                FanLeafResult::Ok {
                    kind,
                    parent_knot,
                    member,
                    entity_id,
                    ..
                } => {
                    add_fanned(&mut run.draft, kind, *parent_knot, *member);
                    pool = host_pool;
                    run.record_conversion(&host_id, host_type_id, kind, &entity_id);
                    converted_this_pass += 1;
                }
                FanLeafResult::Refused { reason, .. } => {
                    use super::fan::FanLeafRefusal as R;
                    let branch = (tip.z >= CONSOLIDATION_BRANCH_MIN_HEIGHT_MM
                        && matches!(reason, R::Blocked | R::Cross | R::SameZ))
                    .then(|| {
                        build_consolidation_branch(
                            tip,
                            tip_normal,
                            model_id,
                            &host_pool,
                            &run.draft,
                            con_fan_radius_mm,
                            auto.max_attachments_per_trunk,
                            &format!("auto-con-branch-{host_id}"),
                            ctx,
                        )
                    })
                    .flatten();
                    if let Some((parent_knot, member, branch_id, kind)) = branch {
                        add_fanned(&mut run.draft, kind, parent_knot, member);
                        pool = host_pool;
                        run.record_conversion(&host_id, host_type_id, kind, &branch_id);
                        converted_this_pass += 1;
                        continue;
                    }
                    bump(&mut con_refusals, fan_key(reason));
                    // The host stays: back where it was.
                    let collections = &mut run.draft.collections;
                    if let Some((index, key, root)) = pruned_root {
                        collections.roots.shift_insert(index, key, root);
                    }
                    if let Some((index, key, trunk)) = pruned_trunk {
                        collections.trunks.shift_insert(index, key, trunk);
                    }
                }
            }
        }
        if converted_this_pass == 0 {
            break;
        }
    }

    perf::measure("auto:consolidation");
    perf::mark("auto:gap-fill");
    // ── Coverage convergence (gap-fill) ─────────────────────────────
    for _ in 0..MAX_GAP_FILL_PASSES {
        // Reads the committed store, not the draft, as DragonFruit does.
        let tips = collect_support_tips(input.committed);
        let mut gap_candidates = build_gap_fill_candidates(&overhang_islands, &auto, &tips);
        if gap_candidates.is_empty() {
            break;
        }
        for c in &mut gap_candidates {
            c.model_id = model_id.to_owned();
        }
        let mut placed_this_pass = 0;
        for c in &gap_candidates {
            if let Some(kind) = run.place(c, model_ctx.as_ref(), ctx)
                && get_support_type_descriptor(kind)
                    .placement_rule
                    .is_some_and(|r| r.metric == SupportPlacementMetric::TipHeight)
            {
                placed_this_pass += 1;
            }
        }
        if placed_this_pass == 0 {
            break;
        }
    }

    perf::measure("auto:gap-fill");
    perf::mark("auto:analytics");
    // ── Coverage analytics ───────────────────────────────────────────
    let IslandCoverage {
        mut supported_ids,
        mut covered_area,
        total_area,
    } = island_coverage(islands, &collect_contact_positions(&run.draft));

    let sizing_debug = model_ctx
        .as_ref()
        .filter(|_| !candidates.is_empty())
        .map(|mctx| {
            let weight_g = mctx.model_volume_mm3 * RESIN_DENSITY_G_PER_MM3;
            let factors = model_sizing_factors(Some(mctx), live);
            let mut areas: Vec<f64> = candidates.iter().map(|c| c.island_area_mm2).collect();
            areas.sort_by(|a, b| js_compare(a - b));
            let min_area = areas[0];
            let max_area = areas[areas.len() - 1];
            let avg_area = areas.iter().fold(0.0, |s, a| s + a) / areas.len() as f64;
            let z_max = candidates.iter().fold(1.0, |m, c| js_max(m, c.z_height));
            let sample = |area: f64, z: f64| CandidatePoint {
                id: "dbg".to_owned(),
                tip_pos: Vec3::new(0.0, 0.0, 0.0),
                tip_normal: Vec3::new(0.0, 0.0, -1.0),
                model_id: String::new(),
                source: CandidateSource::Voxel,
                island_area_mm2: area,
                z_height: z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: None,
            };
            let size_scale = live.auto_support.size_scale;
            let s_min = size_parameters(
                &sample(min_area, 10.0),
                size_scale,
                Some(mctx),
                live,
                ctx.bands,
            );
            let s_max = size_parameters(
                &sample(max_area, z_max),
                size_scale,
                Some(mctx),
                live,
                ctx.bands,
            );
            let s_avg = size_parameters(
                &sample(avg_area, z_max / 2.0),
                size_scale,
                Some(mctx),
                live,
                ctx.bands,
            );
            let hosts = placed_host_count(&run.placed);
            let kinds = run.diagnostics.hosts_by_kind;
            SizingDebugInfo {
                model_volume_mm3: js_round(mctx.model_volume_mm3),
                estimated_weight_g: round2_mm(weight_g),
                total_candidates: mctx.total_candidates,
                weight_per_support_g: round2_mm(if hosts > 0 {
                    weight_g / hosts as f64
                } else {
                    0.0
                }),
                model_size_mm: round2_mm(factors.size_mm),
                load_share_g: round2_mm(factors.load_share_g),
                size_factor: round2_mm(factors.size_factor),
                load_factor: round2_mm(factors.load_factor),
                avg_island_area_mm2: round2_mm(avg_area),
                standalone_hosts: kinds.standalone,
                grid_infill_hosts: kinds.grid_infill + kinds.coverage_fill,
                shaft_diameter_range: SizeRange {
                    min: round2_mm(s_min.shaft_diameter_mm.unwrap_or(0.0)),
                    max: round2_mm(s_max.shaft_diameter_mm.unwrap_or(0.0)),
                    avg: round2_mm(s_avg.shaft_diameter_mm.unwrap_or(0.0)),
                },
                tip_contact_range: SizeRange {
                    min: round2_mm(s_min.tip_contact_diameter_mm.unwrap_or(0.0)),
                    max: round2_mm(s_max.tip_contact_diameter_mm.unwrap_or(0.0)),
                    avg: round2_mm(s_avg.tip_contact_diameter_mm.unwrap_or(0.0)),
                },
            }
        });

    let mut analytics = AutoPlaceAnalytics {
        islands_covered: supported_ids.len() as i64,
        islands_uncovered: islands.len() as i64 - supported_ids.len() as i64,
        presets: run.presets,
        rejection_reasons: run.rejection_reasons.clone(),
        area_coverage: if total_area > 0.0 {
            covered_area / total_area
        } else {
            0.0
        },
        placement: Some(run.diagnostics.clone()),
        sizing_debug,
        forest_report: None,
    };

    perf::measure("auto:analytics");
    perf::mark("auto:fanning");
    // ── Post-placement leaf fanning ─────────────────────────────────
    let fan_radius_mm = js_max(MIN_LEAF_FAN_RADIUS_MM, auto.leaf_fan_radius_mm);
    let fan_max_angle_deg = js_min(
        auto.leaf_fan_max_angle_deg,
        member_max_angle_from_vertical_deg(live),
    );
    let mut pass = 0;
    while pass < MAX_FANNING_PASSES && analytics.islands_uncovered > 0 {
        let shaft_points = collect_fan_shaft_points(&run.draft);
        if shaft_points.is_empty() {
            break;
        }
        let mut fanned_count = 0;
        for island in islands {
            if supported_ids.contains(&island.id) {
                continue;
            }
            let fan = fan_leaf_to_host(
                &FanArgs {
                    target: island.contact,
                    model_id,
                    shaft_points: &shaft_points,
                    grid_host_ids: &run.grid_host_ids,
                    knot_id_prefix: &format!("auto-fan-{}-p{pass}", island.id),
                    fan_radius_mm,
                    grid_host_fan_radius_mm: GRID_HOST_FAN_RADIUS_MM,
                    max_angle_deg: fan_max_angle_deg,
                    max_attachments: auto.max_attachments_per_trunk,
                    draft: &run.draft,
                    origin: Some(if island.source == IslandSource::Overhang {
                        SupportOriginId::Overhang
                    } else {
                        SupportOriginId::Island
                    }),
                    host_order: FanHostOrder::Steepest,
                    rescue_span_override_mm: None,
                },
                ctx,
            );
            let FanLeafResult::Ok {
                kind,
                parent_knot,
                member,
                entity_id,
                ..
            } = fan
            else {
                continue;
            };
            add_fanned(&mut run.draft, kind, *parent_knot, *member);
            fanned_count += 1;
            *run.placed.get_mut(&kind).expect("every type") += 1;
            supported_ids.insert(island.id.clone());
            covered_area += island.area_mm2.unwrap_or(0.0);
            run.forest_ledger.push(ForestLedgerEntry {
                display_id: island.id.clone(),
                kind,
                entity_id,
                area_mm2: island.area_mm2.unwrap_or(0.0),
                z_height: island.contact.z,
                preset: preset_for_area(island.area_mm2.unwrap_or(0.0)),
                band_shaft_mm: ctx.bands.active(live).shaft_diameter_mm,
            });
        }
        if fanned_count == 0 {
            break;
        }
        analytics.islands_covered += fanned_count;
        analytics.islands_uncovered -= fanned_count;
        analytics.area_coverage = if total_area > 0.0 {
            covered_area / total_area
        } else {
            0.0
        };
        pass += 1;
    }

    perf::measure("auto:fanning");
    perf::mark("auto:surface-coverage");
    // ── Overhang surface coverage (flat voxel islands) ──────────────
    const OVERHANG_AREA_THRESHOLD_MM2: f64 = 1.5;
    const OVERHANG_GRID_SPACING_MM: f64 = 2.5;
    let coverage_stub_reach_mm = js_max(MIN_LEAF_FAN_RADIUS_MM, auto.leaf_fan_radius_mm);
    struct StubHost {
        host_type_id: SupportTypeId,
        host_id: String,
        pos: Vec3,
        diameter: f64,
    }
    let stub_hosts: Vec<StubHost> = collect_host_entities(&run.draft)
        .into_iter()
        .filter(|h| h.entity.base.model_id == model_id)
        .filter_map(|h| {
            let last_seg = h.entity.segments.last();
            let knot_pos = last_seg
                .and_then(|s| s.top_joint.as_ref())
                .map(|j| j.pos)
                .or_else(|| h.entity.contact_cone.as_ref().map(|c| c.pos))?;
            Some(StubHost {
                host_type_id: h.host_type_id,
                host_id: h.host_id.to_owned(),
                pos: knot_pos,
                diameter: last_seg.map_or(1.0, |s| s.diameter),
            })
        })
        .collect();
    for island in islands {
        if island.source == IslandSource::Overhang {
            continue;
        }
        let area = island.area_mm2.unwrap_or(0.0);
        let Some(voxels) = island.contact_voxels.as_ref() else {
            continue;
        };
        if area < OVERHANG_AREA_THRESHOLD_MM2 || voxels.count < 3 {
            continue;
        }
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for vi in 0..voxels.count {
            let (vx, vy) = (voxels.x(vi), voxels.y(vi));
            if vx < min_x {
                min_x = vx;
            }
            if vy < min_y {
                min_y = vy;
            }
            if vx > max_x {
                max_x = vx;
            }
            if vy > max_y {
                max_y = vy;
            }
        }
        let width = max_x - min_x;
        let height = max_y - min_y;
        if width < OVERHANG_GRID_SPACING_MM && height < OVERHANG_GRID_SPACING_MM {
            continue;
        }
        let cols = js_max(2.0, js_round(width / OVERHANG_GRID_SPACING_MM)) as usize;
        let rows = js_max(2.0, js_round(height / OVERHANG_GRID_SPACING_MM)) as usize;
        for r in 0..rows {
            for c in 0..cols {
                let gx = min_x + (width * (c as f64 + 0.5)) / cols as f64;
                let gy = min_y + (height * (r as f64 + 0.5)) / rows as f64;
                let mut in_footprint = false;
                let mut cell_z = island.contact.z;
                let mut cell_dist2 = OVERHANG_GRID_SPACING_MM * OVERHANG_GRID_SPACING_MM;
                for vi in 0..voxels.count {
                    let dx = gx - voxels.x(vi);
                    let dy = gy - voxels.y(vi);
                    let d2 = dx * dx + dy * dy;
                    if d2 > cell_dist2 {
                        continue;
                    }
                    cell_dist2 = d2;
                    in_footprint = true;
                    cell_z = voxels.z(vi).unwrap_or(island.contact.z);
                }
                if !in_footprint {
                    continue;
                }
                let c_dist = (gx - island.contact.x).powi(2) + (gy - island.contact.y).powi(2);
                if c_dist < 1.0 {
                    continue;
                }
                let mut host: Option<&StubHost> = None;
                let mut host_dist2 = coverage_stub_reach_mm * coverage_stub_reach_mm;
                for candidate in &stub_hosts {
                    let dx = gx - candidate.pos.x;
                    let dy = gy - candidate.pos.y;
                    let d2 = dx * dx + dy * dy;
                    if d2 > host_dist2 {
                        continue;
                    }
                    host_dist2 = d2;
                    host = Some(candidate);
                }
                let Some(host) = host else {
                    continue;
                };
                if is_host_at_attachment_capacity(
                    host.host_type_id,
                    &host.host_id,
                    auto.max_attachments_per_trunk,
                    &run.draft,
                ) {
                    continue;
                }
                let (point, normal) = resolve_surface_normal(Vec3::new(gx, gy, cell_z), mesh);
                let parent_knot = Knot {
                    id: format!("auto-overhang-{}-{}-{r}-{c}", host.host_id, island.id),
                    parent_shaft_id: host.host_id.clone(),
                    pos: host.pos,
                    diameter: Some(host.diameter + 0.1),
                    ..Knot::default()
                };
                let band = ctx.bands.active(live);
                let Some(built) = build_hosted_branch(
                    &HostedBranchInput {
                        tip_pos: point,
                        tip_normal: normal,
                        model_id,
                        parent_knot: &parent_knot,
                        tip_contact_diameter_mm: Some(band.tip_contact_diameter_mm),
                        roots_diameter_mm: Some(band.root_diameter_mm),
                    },
                    band.shaft_diameter_mm,
                    host.diameter,
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
                let member_type_id = branch
                    .base
                    .type_id
                    .expect("a built member carries its typeId");
                draft_add_primitive(&mut run.draft, PlacementPrimitive::Knot(parent_knot));
                draft_add_entity(
                    &mut run.draft,
                    member_type_id,
                    SupportEntityValue::Branch(branch),
                );
                *run.placed.get_mut(&member_type_id).expect("every type") += 1;
            }
        }
    }

    let changed = run.placed.values().any(|&count| count > 0);

    perf::measure("auto:surface-coverage");
    perf::mark("auto:resize-report");
    // ── 4. Forest resize, rehost, orphan cull, cone sync, report ────
    if changed {
        let resized = compute_forest_diameter_profile(&run.draft, live, &mut || ctx.id());
        run.draft = resized;
        run.draft = rehost_legacy_knots(&run.draft);
        let mut orphan_infos: Vec<OrphanInfo> = Vec::new();
        let pre_cull_draft = run.draft.clone();
        let (culled, orphans) = validate_and_cull_orphans(&run.draft, mesh);
        if !orphans.is_empty() {
            run.draft = culled;
            orphan_infos = orphans;
            // Re-place members orphaned by a culled (blocked) host.
            let requeue: Vec<OrphanInfo> = orphan_infos
                .iter()
                .filter(|o| {
                    o.reason == OrphanReason::MissingHost
                        && o.detail
                            .as_deref()
                            .unwrap_or("")
                            .contains("host trunk culled")
                })
                .cloned()
                .collect();
            if !requeue.is_empty() {
                for o in &requeue {
                    let c = &pre_cull_draft.collections;
                    let member = c
                        .leaves
                        .get(&o.id)
                        .map(|l| (&l.base, Some(&l.contact_cone)))
                        .or_else(|| {
                            c.branches
                                .get(&o.id)
                                .map(|b| (&b.base, b.contact_cone.as_ref()))
                        });
                    let Some((base, Some(cone))) = member else {
                        continue;
                    };
                    let recandidate = CandidatePoint {
                        id: format!("{}-requeue", o.id),
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
                    };
                    let result = place_one_candidate(
                        &recandidate,
                        &mut run.draft,
                        &PlaceArgs {
                            grid_host_ids: Some(&run.grid_host_ids),
                            sizing_ctx: model_ctx.as_ref(),
                        },
                        ctx,
                        false,
                    );
                    match result.kind {
                        None => {
                            run.rejected_count += 1;
                            if let Some(reason) = result.rejected_reason {
                                bump_reason(&mut run.rejection_reasons, reason);
                            }
                        }
                        Some(kind) => *run.placed.get_mut(&kind).expect("every type") += 1,
                    }
                    if let Some(preset) = result.preset {
                        // `analytics.presets` is the run's own object.
                        analytics.presets.bump(preset);
                    }
                    if result.kind.is_some() {
                        run.placed_contacts.push(PlacedContact {
                            tip: recandidate.tip_pos,
                            preset: result
                                .preset
                                .unwrap_or_else(|| preset_for_area(recandidate.island_area_mm2)),
                        });
                    }
                    if let Some(entity_id) = &result.entity_id
                        && is_auto_placed_type(result.kind)
                    {
                        run.forest_ledger.push(ForestLedgerEntry {
                            display_id: recandidate.id.clone(),
                            kind: result.kind.expect("auto-placed"),
                            entity_id: entity_id.clone(),
                            area_mm2: recandidate.island_area_mm2,
                            z_height: recandidate.z_height,
                            preset: result
                                .preset
                                .unwrap_or_else(|| preset_for_area(recandidate.island_area_mm2)),
                            band_shaft_mm: ctx.bands.active(live).shaft_diameter_mm,
                        });
                    }
                }
                let (recheck, rechecked) = validate_and_cull_orphans(&run.draft, mesh);
                // DragonFruit's identity test: the cull returns its input
                // untouched unless it removed something.
                if recheck != run.draft {
                    run.draft = recheck;
                    let reported: HashSet<String> =
                        orphan_infos.iter().map(|o| o.id.clone()).collect();
                    orphan_infos
                        .extend(rechecked.into_iter().filter(|o| !reported.contains(&o.id)));
                }
            }
        }
        run.draft = sync_contact_cone_diameters(&run.draft);

        let mut forest_report = build_forest_report(&run.draft, &run.forest_ledger);
        let mut by_source = BySource::default();
        for island in islands {
            match island.source {
                IslandSource::Voxel => by_source.voxel += 1,
                IslandSource::Minima => by_source.minima += 1,
                IslandSource::Overhang => by_source.overhang += 1,
            }
        }
        forest_report.scan = Some(ForestScanMetrics {
            islands: islands.len(),
            by_source,
            overhang_regions: overhang_islands.len(),
            anchor_clusters: 0,
            anchor_regions: 0,
            candidates: candidates.len(),
            total_area_mm2: islands
                .iter()
                .fold(0.0, |s, i| s + i.area_mm2.unwrap_or(0.0)),
            coverage_percent: analytics.area_coverage * 100.0,
            uncovered_islands: analytics.islands_uncovered.max(0) as usize,
            rejected: run.rejected_count,
            deduped_away: Some(before_dedup - deduped_candidates),
            already_supported: Some(before_support_filter - filtered_candidates),
            rejection_reasons: Some(run.rejection_reasons.clone()),
        });
        if !orphan_infos.is_empty() {
            forest_report.orphans = Some(orphan_infos);
        }
        let d = &run.diagnostics;
        forest_report.diagnostics = Some(ReportDiagnostics {
            candidates_by_source: d.candidates_by_source,
            hosts_by_kind: d.hosts_by_kind,
            fan_refusals: d.fan_refusals.clone(),
            merge_refusals: d.merge_refusals.clone(),
            grid_fallbacks: d.grid_fallbacks,
            consolidation_refusals: con_refusals.clone(),
            cavity_fallbacks: d.cavity_fallbacks.clone(),
        });
        forest_report.load_budget = Some(compute_load_budget(
            &LoadBudgetInput {
                islands,
                contacts: &run.placed_contacts,
                area_per_support_mm2: live.auto_support.area_per_support_mm2,
                pose_drag_moment_mm3: pose_stability.as_ref().map(|p| p.drag_moment_mm3),
                topple_coverage_needed,
            },
            ctx.bands,
        ));
        analytics.forest_report = Some(forest_report);
    }

    perf::measure("auto:resize-report");
    perf::mark("auto:bracing");
    // ── 5. Auto-bracing, folded into the plan ───────────────────────
    if changed && !auto.debug_skip_auto_bracing {
        let (braced, _) = build_auto_braced_snapshot(
            &run.draft,
            &live.auto_bracing,
            live,
            input.brace_meshes,
            input.committed,
            &mut || ctx.id(),
        );
        run.draft = braced;
    }
    perf::measure("auto:bracing");
    perf::mark("auto:final-coverage");
    // ── 6. Coverage of the supports the plan hands back ─────────────
    // The counts above drive the fanning and stop there; the cull may still
    // take tips away, so the analytics report what is left at the end.
    let end = island_coverage(islands, &collect_contact_positions(&run.draft));
    analytics.islands_covered = end.supported_ids.len() as i64;
    analytics.islands_uncovered = islands.len() as i64 - end.supported_ids.len() as i64;
    analytics.area_coverage = end.area_fraction();
    analytics.rejection_reasons = run.rejection_reasons.clone();
    if let Some(scan) = analytics
        .forest_report
        .as_mut()
        .and_then(|f| f.scan.as_mut())
    {
        scan.coverage_percent = analytics.area_coverage * 100.0;
        scan.uncovered_islands = analytics.islands_uncovered.max(0) as usize;
        scan.rejection_reasons = Some(run.rejection_reasons.clone());
    }
    perf::measure("auto:final-coverage");

    let result = AutoPlaceResult {
        placed: run.placed,
        rejected_candidates: run.rejected_count,
        changed,
        status: AutoPlaceStatus::Placed,
        analytics: Some(analytics.clone()),
    };
    Some(AutoSupportPlan {
        before,
        support: run.draft,
        analytics,
        result,
    })
}

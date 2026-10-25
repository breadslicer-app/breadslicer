//! DragonFruit `src/supports/autoSupport/autoPlace.ts`, the forest report: a
//! structured per-run summary of the placed forest (every support's size and
//! sizing reasoning, plus the fan-out groups), with the report types from
//! `types.ts`. `forestReportToText`, the clipboard rendering, is the GUI's.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::fan::FanLeafRefusal;
use super::helpers::{OrphanInfo, collect_host_entities};
use crate::supports::auto_support::load_budget::LoadBudgetReport;
use crate::supports::auto_support::settings::SizingPreset;
use crate::supports::support_type_registry::{
    SHAFT_HOSTED_MEMBER_TYPE_ORDER, SupportEntityValue, SupportTypeId,
};
use crate::supports::support_types::trunk::trunk_replacement::max_connected_diameter::member_diameter_of;
use crate::supports::types::{SupportState, Vec3};
use crate::three::math_utils::{js_atan2, js_hypot, js_max, js_min, js_to_fixed};

/// Per-placed-entity entry in the Forest Report ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForestLedgerEntry {
    pub display_id: String,
    pub kind: SupportTypeId,
    pub entity_id: String,
    pub area_mm2: f64,
    pub z_height: f64,
    pub preset: SizingPreset,
    /// The active profile band's shaft Ø at placement (mm).
    pub band_shaft_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForestMember {
    pub id: String,
    pub kind: SupportTypeId,
    pub span_mm: f64,
    pub angle_deg: f64,
    pub diameter_mm: f64,
}

/// One fan-out group: a host trunk + its attachments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForestTree {
    pub host_id: String,
    pub host_z: f64,
    pub shaft_diameter_mm: f64,
    pub sizing_note: String,
    pub members: Vec<ForestMember>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BareHost {
    pub id: String,
    pub z: f64,
    pub shaft_diameter_mm: f64,
    pub sizing_note: String,
}

/// Candidate counts by detector source.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatesBySource {
    pub voxel: usize,
    pub minima: usize,
    pub intersection: usize,
    pub overhang: usize,
    pub stabilization: usize,
    pub reinforcement: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsByKind {
    pub grid_infill: i64,
    pub coverage_fill: i64,
    pub standalone: i64,
}

/// A candidate whose trunk could not reach the plate and was bridged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CavityFallback {
    pub id: String,
    pub kind: SupportTypeId,
    pub tip: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_refusal: Option<String>,
}

/// Why trunks are where they are, fan/merge refusal counts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementDiagnostics {
    pub candidates_by_source: CandidatesBySource,
    pub hosts_by_kind: HostsByKind,
    pub fan_refusals: IndexMap<String, usize>,
    pub merge_refusals: IndexMap<String, usize>,
    pub grid_fallbacks: usize,
    pub cavity_fallbacks: Vec<CavityFallback>,
}

/// The report's diagnostics block (placement's, plus consolidation refusals).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDiagnostics {
    pub candidates_by_source: CandidatesBySource,
    pub hosts_by_kind: HostsByKind,
    pub fan_refusals: IndexMap<String, usize>,
    pub merge_refusals: IndexMap<String, usize>,
    pub grid_fallbacks: usize,
    pub consolidation_refusals: IndexMap<String, usize>,
    pub cavity_fallbacks: Vec<CavityFallback>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BySource {
    pub voxel: usize,
    pub minima: usize,
    pub intersection: usize,
    pub overhang: usize,
}

/// Input-side metrics from the island/overhang scan.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForestScanMetrics {
    pub islands: usize,
    pub by_source: BySource,
    pub overhang_regions: usize,
    pub anchor_clusters: usize,
    pub anchor_regions: usize,
    pub candidates: usize,
    pub total_area_mm2: f64,
    pub coverage_percent: f64,
    pub uncovered_islands: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deduped_away: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub already_supported: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_reasons: Option<IndexMap<String, usize>>,
    pub rejected: usize,
}

/// Structured per-run summary of the placed forest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForestReport {
    pub host_count: usize,
    pub stump_count: usize,
    pub leaf_count: usize,
    pub branch_count: usize,
    pub stick_count: usize,
    pub twig_count: usize,
    pub trees: Vec<ForestTree>,
    pub bare_hosts: Vec<BareHost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<ForestScanMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphans: Option<Vec<OrphanInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_budget: Option<LoadBudgetReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<ReportDiagnostics>,
}

fn clamp_range(value: f64, min: f64, max: f64) -> f64 {
    js_min(max, js_max(min, value))
}

/// The sizing reasoning for one placed support.
pub fn forest_sizing_note(entry: &ForestLedgerEntry, actual_shaft_mm: f64) -> String {
    let height_factor = 1.0 + clamp_range((entry.z_height - 20.0) / 200.0, 0.0, 0.25);
    let area_input = js_max(entry.area_mm2, 0.01);
    format!(
        "area {}mm² · base Ø{} · h{} → Ø{}mm",
        js_to_fixed(area_input, 2),
        js_to_fixed(entry.band_shaft_mm, 2),
        js_to_fixed(height_factor, 2),
        js_to_fixed(actual_shaft_mm, 2)
    )
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

pub fn build_forest_report(draft: &SupportState, ledger: &[ForestLedgerEntry]) -> ForestReport {
    let mut display_by_entity: IndexMap<&str, &str> = IndexMap::new();
    let mut entry_by_entity: IndexMap<&str, &ForestLedgerEntry> = IndexMap::new();
    for entry in ledger {
        display_by_entity.insert(&entry.entity_id, &entry.display_id);
        entry_by_entity.insert(&entry.entity_id, entry);
    }
    let display = |id: &str| {
        display_by_entity
            .get(id)
            .map_or_else(|| short_id(id), |d| (*d).to_owned())
    };

    // Knots reference their host SEGMENT (or the entity for legacy data).
    let hosts = collect_host_entities(draft);
    let mut host_id_by_shaft: IndexMap<&str, &str> = IndexMap::new();
    for h in &hosts {
        host_id_by_shaft.insert(h.host_id, h.host_id);
        for seg in &h.entity.segments {
            host_id_by_shaft.insert(&seg.id, h.host_id);
        }
    }

    let mut members_by_host: IndexMap<String, Vec<ForestMember>> = IndexMap::new();
    let c = &draft.collections;
    // Registry walk order: a host's leaves come before its branches.
    for type_id in SHAFT_HOSTED_MEMBER_TYPE_ORDER {
        let members: Vec<(String, String, Option<Vec3>, SupportEntityValue)> = match type_id {
            SupportTypeId::Leaf => c
                .leaves
                .values()
                .map(|l| {
                    (
                        l.base.id.clone(),
                        l.parent_knot_id.clone(),
                        Some(l.contact_cone.pos),
                        SupportEntityValue::Leaf(l.clone()),
                    )
                })
                .collect(),
            _ => c
                .branches
                .values()
                .map(|b| {
                    (
                        b.base.id.clone(),
                        b.parent_knot_id.clone(),
                        b.contact_cone.as_ref().map(|k| k.pos),
                        SupportEntityValue::Branch(b.clone()),
                    )
                })
                .collect(),
        };
        for (id, knot_id, tip, entity) in members {
            if knot_id.is_empty() {
                continue;
            }
            let Some(knot) = c.knots.get(&knot_id) else {
                continue;
            };
            let host_id = host_id_by_shaft
                .get(knot.parent_shaft_id.as_str())
                .map_or_else(|| knot.parent_shaft_id.clone(), |h| (*h).to_owned());
            let k = knot.pos;
            let (h_dist, v_dist, span_mm) = match tip {
                Some(t) => (
                    js_hypot(&[t.x - k.x, t.y - k.y]),
                    t.z - k.z,
                    js_hypot(&[t.x - k.x, t.y - k.y, t.z - k.z]),
                ),
                None => (0.0, 0.0, 0.0),
            };
            let angle_deg = if v_dist > 0.01 {
                (js_atan2(h_dist, v_dist) * 180.0) / std::f64::consts::PI
            } else {
                90.0
            };
            members_by_host
                .entry(host_id)
                .or_default()
                .push(ForestMember {
                    id: display(&id),
                    kind: type_id,
                    span_mm,
                    angle_deg,
                    diameter_mm: member_diameter_of(&entity),
                });
        }
    }

    let mut trees: Vec<ForestTree> = Vec::new();
    let mut bare_hosts: Vec<BareHost> = Vec::new();
    for h in &hosts {
        let shaft_mm = h.entity.segments.first().map_or(0.0, |s| s.diameter);
        let note = entry_by_entity
            .get(h.host_id)
            .map_or_else(String::new, |e| forest_sizing_note(e, shaft_mm));
        let z = h.entity.contact_cone.as_ref().map_or(0.0, |c| c.pos.z);
        match members_by_host.get(h.host_id).filter(|m| !m.is_empty()) {
            Some(members) => trees.push(ForestTree {
                host_id: display(h.host_id),
                host_z: z,
                shaft_diameter_mm: shaft_mm,
                sizing_note: note,
                members: members.clone(),
            }),
            None => bare_hosts.push(BareHost {
                id: display(h.host_id),
                z,
                shaft_diameter_mm: shaft_mm,
                sizing_note: note,
            }),
        }
    }
    trees.sort_by_key(|t| std::cmp::Reverse(t.members.len()));
    bare_hosts.sort_by(|a, b| {
        (a.z - b.z)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    ForestReport {
        host_count: hosts.len(),
        stump_count: c.stumps.len(),
        leaf_count: c.leaves.len(),
        branch_count: c.branches.len(),
        stick_count: c.sticks.len(),
        twig_count: c.twigs.len(),
        trees,
        bare_hosts,
        scan: None,
        orphans: None,
        load_budget: None,
        diagnostics: None,
    }
}

/// A refusal count map, in first-seen order like DragonFruit's object.
pub fn bump(map: &mut IndexMap<String, usize>, key: &str) {
    *map.entry(key.to_owned()).or_insert(0) += 1;
}

/// `FanLeafRefusal` as the diagnostics key.
pub fn fan_key(r: FanLeafRefusal) -> &'static str {
    r.as_str()
}

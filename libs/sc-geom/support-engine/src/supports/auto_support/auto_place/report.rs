//! What a run tells about itself: the supports culled and placed again,
//! and how placement went (by source, by kind of host, refusals, bridges).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::fan::FanLeafRefusal;
use super::helpers::OrphanInfo;
use crate::supports::support_kinds::SupportTypeId;
use crate::supports::types::Vec3;

/// What a run reports about its forest.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForestReport {
    /// The supports the final check took off, and whether each was placed
    /// again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphans: Option<Vec<OrphanInfo>>,
}

/// Candidates by what made them.
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

/// A candidate whose trunk could not reach the plate, bridged instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CavityFallback {
    pub id: String,
    pub kind: SupportTypeId,
    pub tip: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_refusal: Option<String>,
}

/// How placement went.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementDiagnostics {
    pub candidates_by_source: CandidatesBySource,
    pub hosts_by_kind: HostsByKind,
    pub fan_refusals: IndexMap<String, usize>,
    pub merge_refusals: IndexMap<String, usize>,
    pub grid_fallbacks: usize,
    pub cavity_fallbacks: Vec<CavityFallback>,
    /// Why the lattice could not serve each fallback (#463).
    #[serde(skip)]
    pub grid_fallback_reasons: IndexMap<String, usize>,
    /// Rejections by zone size and reason, as `<size>:<reason>` (#463).
    #[serde(skip)]
    pub rejections_by_zone: IndexMap<String, usize>,
}

pub fn bump(map: &mut IndexMap<String, usize>, key: &str) {
    *map.entry(key.to_owned()).or_insert(0) += 1;
}

/// `FanLeafRefusal` as the diagnostics key.
pub fn fan_key(r: FanLeafRefusal) -> &'static str {
    r.as_str()
}

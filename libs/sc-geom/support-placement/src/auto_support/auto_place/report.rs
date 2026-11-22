//! What a run tells about itself: the supports culled and placed again,
//! and how placement went (rejections, contacts off the lattice, bridges).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::helpers::OrphanInfo;
use support_model::support_kinds::SupportTypeId;
use support_model::types::Vec3;

/// What a run reports about its forest.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForestReport {
    /// The supports the final check took off, and whether each was placed
    /// again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphans: Option<Vec<OrphanInfo>>,
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
    pub grid_fallbacks: usize,
    pub cavity_fallbacks: Vec<CavityFallback>,
    /// Why the lattice could not serve each fallback (#463).
    #[serde(skip)]
    pub grid_fallback_reasons: IndexMap<String, usize>,
    /// Rejections by zone size and reason, as `<size>:<reason>` (#463).
    #[serde(skip)]
    pub rejections_by_zone: IndexMap<String, usize>,
}

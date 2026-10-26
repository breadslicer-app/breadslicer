//! Editing supports after a run (SC-10/SC-12, first milestone): a support is
//! added where the person clicks the model, routed by DragonFruit's own click
//! path (`build_trunk_data_routed`, the V3 router), and a support is removed
//! where they click it, with everything that hangs from it. The session keeps
//! the run's models, settings and support state, so each edit re-tessellates
//! the whole forest the way a run does.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use serde_json::Value;
use support_gen_df::pipeline::{ModelInput, PipelineSettings, support_soup, world_soup};
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::placement_logic_v3::smart_placement_v3::RouterStats;
use support_gen_df::supports::support_types::trunk::trunk_builder::{
    TrunkBuildInput, TrunkRouter, build_trunk_data_routed,
};
use support_gen_df::supports::types::{SupportCollections, SupportState};
use support_gen_df::three::{Matrix4, Vec3};
use support_gen_df::three_mesh_bvh::MeshBvh;

use crate::{DF_SUPPORT_MESH_NAME, weld_parts};

/// How far from the model surface a click may land and still place a support.
const ADD_REACH_MM: f64 = 2.0;
/// How far from a support a click may land and still remove it.
const REMOVE_REACH_MM: f64 = 3.0;

/// The supports of a run, kept for editing.
#[derive(Debug, Clone)]
pub struct DfSupportSession {
    models: Vec<ModelInput>,
    settings: PipelineSettings,
    state: SupportState,
    /// Per model, its world mesh, built on the first edit.
    bvhs: Vec<Option<Arc<MeshBvh>>>,
    id_prefix: String,
    next_id: u64,
}

/// Why an edit did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// Nothing to act on within reach of the click.
    NothingThere,
    /// DragonFruit's router found no way down from there.
    NoRoute(String),
    /// The tessellation failed (a raft Clipper error).
    Tessellation(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NothingThere => write!(f, "nothing within reach of the click"),
            EditError::NoRoute(why) => write!(f, "no route to the plate from there ({why})"),
            EditError::Tessellation(why) => write!(f, "the supports could not be drawn ({why})"),
        }
    }
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_dist(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if len2 <= 1e-18 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / len2)
        .clamp(0.0, 1.0);
    dist(p, [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t])
}

/// Every `{x, y, z}` point in a serialized entity, in document order: tips,
/// joints, bases, control points.
fn points_of(value: &Value, out: &mut Vec<[f64; 3]>) {
    match value {
        Value::Object(map) => {
            if let (Some(x), Some(y), Some(z)) = (
                map.get("x").and_then(Value::as_f64),
                map.get("y").and_then(Value::as_f64),
                map.get("z").and_then(Value::as_f64),
            ) {
                out.push([x, y, z]);
            }
            for v in map.values() {
                points_of(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                points_of(v, out);
            }
        }
        _ => {}
    }
}

/// Every entity of the collections as `(id, serialized)`.
fn entities(c: &SupportCollections) -> Vec<(String, Value)> {
    fn add<T: serde::Serialize>(
        out: &mut Vec<(String, Value)>,
        map: &indexmap::IndexMap<String, T>,
    ) {
        for (id, e) in map {
            if let Ok(v) = serde_json::to_value(e) {
                out.push((id.clone(), v));
            }
        }
    }
    let mut out = Vec::new();
    add(&mut out, &c.roots);
    add(&mut out, &c.trunks);
    add(&mut out, &c.branches);
    add(&mut out, &c.leaves);
    add(&mut out, &c.twigs);
    add(&mut out, &c.sticks);
    add(&mut out, &c.braces);
    add(&mut out, &c.stumps);
    out
}

fn remove_ids(c: &mut SupportCollections, ids: &std::collections::HashSet<String>) {
    c.roots.retain(|id, _| !ids.contains(id));
    c.trunks.retain(|id, _| !ids.contains(id));
    c.branches.retain(|id, _| !ids.contains(id));
    c.leaves.retain(|id, _| !ids.contains(id));
    c.twigs.retain(|id, _| !ids.contains(id));
    c.sticks.retain(|id, _| !ids.contains(id));
    c.braces.retain(|id, _| !ids.contains(id));
    c.stumps.retain(|id, _| !ids.contains(id));
}

/// Whether `value` names one of `ids` anywhere in its strings.
fn references(value: &Value, ids: &std::collections::HashSet<String>) -> bool {
    match value {
        Value::String(s) => ids.contains(s),
        Value::Object(map) => map.values().any(|v| references(v, ids)),
        Value::Array(items) => items.iter().any(|v| references(v, ids)),
        _ => false,
    }
}

impl DfSupportSession {
    pub(crate) fn new(
        models: Vec<ModelInput>,
        settings: PipelineSettings,
        state: SupportState,
        id_prefix: String,
    ) -> Self {
        let bvhs = vec![None; models.len()];
        DfSupportSession {
            models,
            settings,
            state,
            bvhs,
            id_prefix,
            next_id: 1_000_000,
        }
    }

    /// The support state as it now stands.
    pub fn state(&self) -> &SupportState {
        &self.state
    }

    fn bvh(&mut self, i: usize) -> Arc<MeshBvh> {
        if let Some(b) = &self.bvhs[i] {
            return b.clone();
        }
        let soup = world_soup(&self.models[i]);
        let triples: Vec<[f32; 3]> = soup.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        let bvh = Arc::new(MeshBvh::new(triples, None));
        self.bvhs[i] = Some(bvh.clone());
        bvh
    }

    /// Adds a support whose tip touches the model at `point` (world mm), the
    /// way a click in DragonFruit's Support Studio does.
    pub fn add_trunk_at(&mut self, point: [f64; 3]) -> Result<(), EditError> {
        let p = Vec3::new(point[0], point[1], point[2]);
        let mut best: Option<(usize, f64, Vec3, Vec3)> = None;
        for i in 0..self.models.len() {
            let bvh = self.bvh(i);
            let Some(hit) = bvh.closest_point_to_point(p, 0.0, ADD_REACH_MM) else {
                continue;
            };
            if hit.distance > ADD_REACH_MM || best.is_some_and(|b| b.1 <= hit.distance) {
                continue;
            }
            let tri = bvh.triangle(hit.face_index);
            let n = Vec3::sub_vectors(tri.b, tri.a).cross(Vec3::sub_vectors(tri.c, tri.a));
            if n.length() <= 1e-12 {
                continue;
            }
            best = Some((i, hit.distance, hit.point, n.normalize()));
        }
        let Some((i, _, tip, normal)) = best else {
            return Err(EditError::NothingThere);
        };
        let bvh = self.bvh(i);
        let mut sdf = SdfCache::new(bvh, Matrix4::IDENTITY, Some(self.settings.sdf_cell_mm));
        let mut stats = RouterStats::default();
        let model_id = self.models[i].id.clone();
        let prefix = self.id_prefix.clone();
        let mut next = self.next_id;
        let mut new_id = || {
            next += 1;
            format!("{prefix}-m{next}")
        };
        let built = build_trunk_data_routed(
            &TrunkBuildInput {
                tip_pos: tip,
                tip_normal: normal,
                model_id: &model_id,
                mesh: None,
                is_preview: Some(false),
                overrides: None,
                is_small_island: None,
                ignore_grid: None,
            },
            &mut TrunkRouter {
                sdf: &mut sdf,
                matrix_world: Matrix4::IDENTITY,
                stats: &mut stats,
            },
            &self.settings.supports,
            &mut new_id,
        );
        self.next_id = next;
        if let Some(error) = built.error {
            return Err(EditError::NoRoute(format!("{error:?}")));
        }
        let c = &mut self.state.collections;
        c.roots.insert(built.root.base.id.clone(), built.root);
        c.trunks.insert(built.trunk.base.id.clone(), built.trunk);
        Ok(())
    }

    /// Removes the support nearest `point` (world mm) and everything that
    /// refers to it (its roots, members hosted on it, braces to it).
    /// Returns how many entities went.
    pub fn remove_near(&mut self, point: [f64; 3]) -> Result<usize, EditError> {
        let all = entities(&self.state.collections);
        let mut best: Option<(f64, &str)> = None;
        for (id, value) in &all {
            let mut pts = Vec::new();
            points_of(value, &mut pts);
            let d = pts
                .windows(2)
                .map(|w| segment_dist(point, w[0], w[1]))
                .chain(pts.iter().map(|q| dist(point, *q)))
                .fold(f64::INFINITY, f64::min);
            if d < best.map_or(f64::INFINITY, |b| b.0) {
                best = Some((d, id));
            }
        }
        let Some((d, id)) = best else {
            return Err(EditError::NothingThere);
        };
        if d > REMOVE_REACH_MM {
            return Err(EditError::NothingThere);
        }
        let mut gone: std::collections::HashSet<String> = std::iter::once(id.to_owned()).collect();
        loop {
            let more: Vec<String> = all
                .iter()
                .filter(|(other, value)| !gone.contains(other) && references(value, &gone))
                .map(|(other, _)| other.clone())
                .collect();
            if more.is_empty() {
                break;
            }
            gone.extend(more);
        }
        // Roots are owned the other way round (a trunk names its root): a
        // root nothing left refers to goes too.
        let remaining: Vec<(&String, &Value)> = all
            .iter()
            .filter(|(other, _)| !gone.contains(other))
            .map(|(id, v)| (id, v))
            .collect();
        let orphans: Vec<String> = self
            .state
            .collections
            .roots
            .keys()
            .filter(|root| !gone.contains(*root))
            .filter(|root| {
                let single: std::collections::HashSet<String> =
                    std::iter::once((*root).clone()).collect();
                let was_used = all
                    .iter()
                    .any(|(other, v)| gone.contains(other) && references(v, &single));
                // A root's own record names it: only others count.
                was_used
                    && !remaining
                        .iter()
                        .any(|(other, v)| *other != *root && references(v, &single))
            })
            .cloned()
            .collect();
        gone.extend(orphans);
        remove_ids(&mut self.state.collections, &gone);
        Ok(gone.len())
    }

    /// The supports and raft as one welded mesh, as a run hands them back;
    /// `None` when nothing is left.
    pub fn mesh(&self) -> Result<Option<IndexedMesh>, EditError> {
        // The rafts are rebuilt from the roots inside the tessellation.
        let sink = support_soup(&self.state, &self.models, &self.settings)
            .map_err(|e| EditError::Tessellation(format!("{e:?}")))?;
        Ok(weld_parts(&sink.floats, &sink.parts, DF_SUPPORT_MESH_NAME))
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn segment_distance_is_to_the_nearest_point() {
        assert!(
            (segment_dist([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]) - 1.0).abs() < 1e-12
        );
        assert!(
            (segment_dist([3.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]) - 2.0).abs() < 1e-12
        );
    }

    #[test]
    fn points_are_found_at_any_depth() {
        let v = serde_json::json!({"tip": {"x": 1.0, "y": 2.0, "z": 3.0}, "joints": [{"x": 0, "y": 0, "z": 1}]});
        let mut pts = Vec::new();
        points_of(&v, &mut pts);
        assert_eq!(pts.len(), 2);
    }
}

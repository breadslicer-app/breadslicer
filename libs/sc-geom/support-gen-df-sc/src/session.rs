//! Editing supports after a run (SC-10/SC-12): a support is added where the
//! person clicks the model the way DragonFruit's auto-placer places one
//! contact (`place_one_candidate`): a fan or a merge onto a nearby support,
//! its own routed trunk, a grid attachment to a neighbour, the gridless
//! fallback. A support is removed where they click it, with everything that
//! hangs from it. Previews of both run on a copy and draw only the pieces
//! involved: green for a placement that works, red for one that does not
//! (a straight trunk where the router found no way down), red for what a
//! removal takes.
//!
//! The session keeps the run's models, settings and support state; the
//! models and their BVHs are shared, so copies for a preview are cheap.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use sc_mesh_core::IndexedMesh;
use serde_json::Value;
use support_gen_df::pipeline::{ModelInput, PipelineSettings, support_soup, world_soup};
use support_gen_df::supports::auto_support::auto_place::helpers::{PlanCtx, PlanMesh};
use support_gen_df::supports::auto_support::auto_place::place::{PlaceArgs, place_one_candidate};
use support_gen_df::supports::auto_support::parameter_sizing::SizingBands;
use support_gen_df::supports::auto_support::types::{CandidatePoint, CandidateSource};
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::rafts::crenelated::raft_types::RaftBottomMode;
use support_gen_df::supports::support_types::trunk::trunk_builder::{
    TrunkBuildInput, build_trunk_data,
};
use support_gen_df::supports::types::{SupportCollections, SupportState};
use support_gen_df::three::{Matrix4, Vec3};
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};

use crate::{DF_SUPPORT_MESH_NAME, weld_parts};

/// How far from the model surface a click may land and still place a support.
const ADD_REACH_MM: f64 = 2.0;
/// How far from a support a click may land and still remove it.
const REMOVE_REACH_MM: f64 = 3.0;

/// The supports of a run, kept for editing.
#[derive(Debug, Clone)]
pub struct DfSupportSession {
    models: Arc<[ModelInput]>,
    settings: Arc<PipelineSettings>,
    state: SupportState,
    /// Per model, its world mesh, built on the first edit and shared by
    /// every copy of the session.
    bvhs: Arc<[OnceLock<Arc<MeshBvh>>]>,
    /// Per model, its distance field. It fills lazily as the router asks,
    /// so keeping it between edits is what makes the second preview fast
    /// (a cold field costs ~200 ms per placement on a 1.4M-face model, a warm
    /// one a few ms, as in a run). Shared by every copy; an edit borrows it
    /// and puts it back, and one finding it lent out builds its own.
    sdfs: Arc<[SdfSlot]>,
    id_prefix: String,
    next_id: u64,
}

/// A model's pooled distance field, while no edit holds it.
#[derive(Default)]
struct SdfSlot(std::sync::Mutex<Option<SdfCache>>);

impl std::fmt::Debug for SdfSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SdfSlot")
    }
}

/// Why an edit did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// Nothing to act on within reach of the click.
    NothingThere,
    /// DragonFruit's placement found no way to hold that point.
    NoRoute(String),
    /// The tessellation failed (a raft Clipper error).
    Tessellation(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NothingThere => write!(f, "nothing within reach of the click"),
            EditError::NoRoute(why) => {
                write!(f, "no way to support that point ({why})")
            }
            EditError::Tessellation(why) => write!(f, "the supports could not be drawn ({why})"),
        }
    }
}

/// What an edit would do, drawn on its own.
#[derive(Debug, Clone)]
pub struct EditPreview {
    /// The pieces the edit adds (or, for a removal, takes away).
    pub mesh: IndexedMesh,
    /// The edit would go through.
    pub valid: bool,
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

/// Every `"id"` inside a serialized entity: its own and its parts'
/// (segments, joints), which other entities attach to.
fn ids_within(value: &Value, out: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(id)) = map.get("id") {
                out.insert(id.clone());
            }
            for v in map.values() {
                ids_within(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                ids_within(v, out);
            }
        }
        _ => {}
    }
}

/// Every entity of the collections as `(id, serialized)`. Knots are listed
/// too: they carry a member on a host shaft and go with either.
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
    add(&mut out, &c.kickstands);
    add(&mut out, &c.knots);
    out
}

/// Every entity id of the collections.
fn all_ids(c: &SupportCollections) -> HashSet<String> {
    let mut ids = HashSet::new();
    ids.extend(c.roots.keys().cloned());
    ids.extend(c.trunks.keys().cloned());
    ids.extend(c.branches.keys().cloned());
    ids.extend(c.leaves.keys().cloned());
    ids.extend(c.twigs.keys().cloned());
    ids.extend(c.sticks.keys().cloned());
    ids.extend(c.braces.keys().cloned());
    ids.extend(c.stumps.keys().cloned());
    ids.extend(c.kickstands.keys().cloned());
    ids.extend(c.knots.keys().cloned());
    ids
}

/// The collections with only the entities `keep` says.
fn filtered(c: &SupportCollections, keep: impl Fn(&String) -> bool) -> SupportCollections {
    let mut out = c.clone();
    out.roots.retain(|id, _| keep(id));
    out.trunks.retain(|id, _| keep(id));
    out.branches.retain(|id, _| keep(id));
    out.leaves.retain(|id, _| keep(id));
    out.twigs.retain(|id, _| keep(id));
    out.sticks.retain(|id, _| keep(id));
    out.braces.retain(|id, _| keep(id));
    out.stumps.retain(|id, _| keep(id));
    out.kickstands.retain(|id, _| keep(id));
    out.knots.retain(|id, _| keep(id));
    out
}

/// Every string inside a serialized entity.
fn strings_in(value: &Value, out: &mut HashSet<String>) {
    match value {
        Value::String(s) => {
            out.insert(s.clone());
        }
        Value::Object(map) => map.values().for_each(|v| strings_in(v, out)),
        Value::Array(items) => items.iter().for_each(|v| strings_in(v, out)),
        _ => {}
    }
}

/// The contact area each sizing tier stands for: DragonFruit sizes an auto
/// support from its island's area, and a click has no island.
fn area_for_sizing(sizing: &str) -> f64 {
    match sizing {
        "detail" => 0.1,
        "anchor" => 1.0,
        _ => 0.3,
    }
}

impl DfSupportSession {
    pub(crate) fn new(
        models: Vec<ModelInput>,
        settings: PipelineSettings,
        state: SupportState,
        id_prefix: String,
    ) -> Self {
        let bvhs = (0..models.len()).map(|_| OnceLock::new()).collect();
        let sdfs = (0..models.len()).map(|_| SdfSlot::default()).collect();
        DfSupportSession {
            models: models.into(),
            settings: Arc::new(settings),
            state,
            bvhs,
            sdfs,
            id_prefix,
            next_id: 1_000_000,
        }
    }

    /// The support state as it now stands.
    pub fn state(&self) -> &SupportState {
        &self.state
    }

    /// Builds every model's BVH now (they are shared by every copy).
    pub fn warm(&self) {
        for i in 0..self.models.len() {
            self.bvh(i);
        }
    }

    /// Hands the run's distance fields to the session, already filled by
    /// the run's routing, so hand edits start warm.
    pub(crate) fn adopt_sdfs(&self, fields: Vec<(usize, SdfCache)>) {
        for (i, field) in fields {
            if let Some(slot) = self.sdfs.get(i)
                && let Ok(mut slot) = slot.0.lock()
            {
                *slot = Some(field);
            }
        }
    }

    fn bvh(&self, i: usize) -> Arc<MeshBvh> {
        self.bvhs[i]
            .get_or_init(|| {
                let soup = world_soup(&self.models[i]);
                let triples: Vec<[f32; 3]> =
                    soup.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                Arc::new(MeshBvh::new(triples, None))
            })
            .clone()
    }

    /// The model surface point nearest `point` within reach: the model, the
    /// point and its face normal.
    fn contact(&self, point: [f64; 3]) -> Option<(usize, Vec3, Vec3)> {
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
        best.map(|(i, _, p, n)| (i, p, n))
    }

    fn id_source(&mut self) -> impl FnMut() -> String + '_ {
        let prefix = self.id_prefix.clone();
        let next = &mut self.next_id;
        move || {
            *next += 1;
            format!("{prefix}-m{next}")
        }
    }

    /// Places a support whose tip touches the model at `point` (world mm).
    /// Returns the ids of what it added.
    fn place(&mut self, point: [f64; 3]) -> Result<HashSet<String>, EditError> {
        let Some((i, tip, normal)) = self.contact(point) else {
            return Err(EditError::NothingThere);
        };
        let bvh = self.bvh(i);
        let settings = self.settings.clone();
        let model_id = self.models[i].id.clone();
        let before = all_ids(&self.state.collections);
        let pooled = self.sdfs[i].0.lock().ok().and_then(|mut slot| slot.take());
        let sdf = RefCell::new(pooled.unwrap_or_else(|| {
            SdfCache::new(bvh.clone(), Matrix4::IDENTITY, Some(settings.sdf_cell_mm))
        }));
        let bands = SizingBands::factory();
        let mut state = std::mem::take(&mut self.state);
        let outcome = {
            let mut new_id = self.id_source();
            let ctx = PlanCtx::new(
                &settings.supports,
                &bands,
                Some(PlanMesh {
                    bvh: &bvh,
                    matrix: Matrix4::IDENTITY,
                    side: Side::Front,
                    normals: None,
                    sdf: &sdf,
                    blocked: None,
                }),
                &mut new_id,
            );
            let candidate = CandidatePoint {
                id: "manual".to_owned(),
                tip_pos: tip,
                tip_normal: normal,
                model_id,
                source: CandidateSource::Overhang,
                island_area_mm2: area_for_sizing(&settings.supports.auto_support.sizing_preset),
                z_height: tip.z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: None,
            };
            let no_grid_hosts = HashSet::new();
            place_one_candidate(
                &candidate,
                &mut state,
                &PlaceArgs {
                    grid_host_ids: Some(&no_grid_hosts),
                    sizing_ctx: None,
                },
                &ctx,
                false,
            )
        };
        self.state = state;
        if let Ok(mut slot) = self.sdfs[i].0.lock() {
            *slot = Some(sdf.into_inner());
        }
        if outcome.kind.is_none() {
            let why = outcome
                .rejected_reason
                .map_or_else(|| "rejected".to_owned(), |r| format!("{r:?}"));
            return Err(EditError::NoRoute(why));
        }
        Ok(all_ids(&self.state.collections)
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect())
    }

    /// Adds a support whose tip touches the model at `point` (world mm), the
    /// way DragonFruit places one contact.
    pub fn add_trunk_at(&mut self, point: [f64; 3]) -> Result<(), EditError> {
        self.place(point).map(|_| ())
    }

    /// What adding a support at `point` would add: green when it goes
    /// through, red (a straight trunk to the plate) when nothing can hold
    /// that point. `NothingThere` when the point is not on a model.
    pub fn preview_add_at(&self, point: [f64; 3]) -> Result<EditPreview, EditError> {
        self.preview_add(point).map(|(preview, _)| preview)
    }

    /// [`Self::preview_add_at`], with the session the click would leave when
    /// it goes through: a click on the same point can take it as is instead
    /// of placing again.
    pub fn preview_add(&self, point: [f64; 3]) -> Result<(EditPreview, Option<Self>), EditError> {
        let mut copy = self.clone();
        match copy.place(point) {
            Ok(added) => {
                let preview = EditPreview {
                    mesh: copy.mesh_of(|id| added.contains(id))?,
                    valid: true,
                };
                Ok((preview, Some(copy)))
            }
            Err(EditError::NoRoute(_)) => {
                let (i, tip, normal) = self.contact(point).ok_or(EditError::NothingThere)?;
                let model_id = self.models[i].id.clone();
                let mut copy = self.clone();
                let built = {
                    let mut new_id = copy.id_source();
                    build_trunk_data(
                        &TrunkBuildInput {
                            tip_pos: tip,
                            tip_normal: normal,
                            model_id: &model_id,
                            mesh: None,
                            is_preview: Some(true),
                            overrides: None,
                            is_small_island: None,
                            ignore_grid: Some(true),
                        },
                        &self.settings.supports,
                        &mut new_id,
                    )
                }
                .ok_or(EditError::NothingThere)?;
                let ids: HashSet<String> =
                    [built.root.base.id.clone(), built.trunk.base.id.clone()].into();
                copy.state
                    .collections
                    .roots
                    .insert(built.root.base.id.clone(), built.root);
                copy.state
                    .collections
                    .trunks
                    .insert(built.trunk.base.id.clone(), built.trunk);
                let preview = EditPreview {
                    mesh: copy.mesh_of(|id| ids.contains(id))?,
                    valid: false,
                };
                Ok((preview, None))
            }
            Err(other) => Err(other),
        }
    }

    /// The entities a click at `point` removes: the nearest one within reach
    /// and everything that hangs from it (members on its shafts and the
    /// knots carrying them, braces to it, a root left with nothing on it).
    fn removal(&self, point: [f64; 3]) -> Result<HashSet<String>, EditError> {
        let all = entities(&self.state.collections);
        let mut best: Option<(f64, &str)> = None;
        for (id, value) in &all {
            if self.state.collections.knots.contains_key(id) {
                continue;
            }
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
        // Who names what, once: every string an entity holds, and the ids of
        // its parts (a knot names the shaft segment it sits on). The cascade
        // and the orphan check then look names up instead of rescanning every
        // entity per step (thousands of entities on a busy model).
        let index_of: std::collections::HashMap<&str, usize> = all
            .iter()
            .enumerate()
            .map(|(k, (id, _))| (id.as_str(), k))
            .collect();
        let mut named_by: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for (k, (_, value)) in all.iter().enumerate() {
            let mut names = HashSet::new();
            strings_in(value, &mut names);
            for name in names {
                named_by.entry(name).or_default().push(k);
            }
        }
        let start = index_of[id];
        let mut gone = vec![false; all.len()];
        gone[start] = true;
        let mut queue = vec![start];
        while let Some(g) = queue.pop() {
            let mut parts = HashSet::new();
            ids_within(&all[g].1, &mut parts);
            parts.insert(all[g].0.clone());
            for part in parts {
                for &k in named_by.get(&part).into_iter().flatten() {
                    if !gone[k] {
                        gone[k] = true;
                        queue.push(k);
                    }
                }
            }
        }
        // Roots and knots are owned the other way round (a trunk names its
        // root, a member its knot): one a removed entity named that nothing
        // left names goes too.
        let c = &self.state.collections;
        let mut orphans = Vec::new();
        for g in (0..all.len()).filter(|&g| gone[g]) {
            let mut names = HashSet::new();
            strings_in(&all[g].1, &mut names);
            for name in names {
                let Some(&k) = index_of.get(name.as_str()) else {
                    continue;
                };
                let owned = c.roots.contains_key(&name) || c.knots.contains_key(&name);
                if gone[k] || !owned {
                    continue;
                }
                // Its own record names it: only others count.
                let still_used = named_by
                    .get(&name)
                    .into_iter()
                    .flatten()
                    .any(|&other| other != k && !gone[other]);
                if !still_used {
                    orphans.push(k);
                }
            }
        }
        for k in orphans {
            gone[k] = true;
        }
        Ok((0..all.len())
            .filter(|&k| gone[k])
            .map(|k| all[k].0.clone())
            .collect())
    }

    /// Removes the support nearest `point` (world mm) and everything that
    /// hangs from it. Returns how many entities went.
    pub fn remove_near(&mut self, point: [f64; 3]) -> Result<usize, EditError> {
        let gone = self.removal(point)?;
        self.state.collections = filtered(&self.state.collections, |id| !gone.contains(id));
        Ok(gone.len())
    }

    /// What a removal at `point` would take away, drawn on its own.
    pub fn preview_remove_at(&self, point: [f64; 3]) -> Result<EditPreview, EditError> {
        let gone = self.removal(point)?;
        Ok(EditPreview {
            mesh: self.mesh_of(|id| gone.contains(id))?,
            valid: false,
        })
    }

    /// The entities `keep` names, tessellated without a raft.
    fn mesh_of(&self, keep: impl Fn(&String) -> bool) -> Result<IndexedMesh, EditError> {
        let state = SupportState {
            collections: filtered(&self.state.collections, keep),
            ..SupportState::default()
        };
        let mut settings = (*self.settings).clone();
        settings.rafts.global.bottom_mode = RaftBottomMode::Off;
        settings.rafts.per_model.clear();
        let sink = support_soup(&state, &self.models, &settings)
            .map_err(|e| EditError::Tessellation(format!("{e:?}")))?;
        weld_parts(&sink.floats, &sink.parts, DF_SUPPORT_MESH_NAME).ok_or(EditError::NothingThere)
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

    #[test]
    fn part_ids_are_found_at_any_depth() {
        let v =
            serde_json::json!({"id": "t1", "segments": [{"id": "s1", "topJoint": {"id": "j1"}}]});
        let mut ids = HashSet::new();
        ids_within(&v, &mut ids);
        assert_eq!(ids.len(), 3);
    }
}

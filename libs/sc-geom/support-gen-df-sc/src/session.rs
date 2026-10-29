//! Editing supports after a run (BS-1, BS-3): a support is added where the
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

use crate::shift::{PlaneMotion, bases, model_collections, move_model_supports};
use crate::{DF_SUPPORT_MESH_NAME, SettingsRecipe, weld_parts};

/// How far from the model surface a click may land and still place a support.
const ADD_REACH_MM: f64 = 2.0;
/// How far a hand-placed support reaches for a support to hang from.
const MANUAL_ATTACH_REACH_MM: f64 = 15.0;
/// How far from a support a click may land and still remove it.
const REMOVE_REACH_MM: f64 = 3.0;
/// How much lower [`DfSupportSession::add_trunk_near`] may move a point the
/// engine cannot hold: a point a layer or two up the island's underside.
const NUDGE_DOWN_MM: [f64; 4] = [0.05, 0.1, 0.2, 0.4];
/// The directions [`DfSupportSession::add_trunk_near`] moves a point sideways.
const NUDGE_DIRECTIONS: usize = 8;

/// The supports of a run, kept for editing.
#[derive(Debug, Clone)]
pub struct DfSupportSession {
    models: Arc<[ModelInput]>,
    settings: Arc<PipelineSettings>,
    /// What `settings` were made from, for saving the session as data;
    /// `None` when the run was given its settings outright.
    recipe: Option<Arc<SettingsRecipe>>,
    state: SupportState,
    /// Per model, its world mesh, built on the first edit and shared by
    /// every copy of the session.
    bvhs: Arc<[OnceLock<Arc<MeshBvh>>]>,
    /// Who names what among the supports, for removals: built on the first
    /// removal or its preview and kept while the state stands (a hover over
    /// the supports asks for it on every move). An edit drops it.
    removal_index: Arc<OnceLock<Arc<RemovalIndex>>>,
    /// Per model, its distance field. It fills lazily as the router asks,
    /// so keeping it between edits is what makes the second preview fast
    /// (a cold field costs ~200 ms per placement on a 1.4M-face model, a warm
    /// one a few ms, as in a run). Shared by every copy; an edit borrows it
    /// and puts it back, and one finding it lent out builds its own.
    sdfs: Arc<[SdfSlot]>,
    id_prefix: String,
    next_id: u64,
    /// Whether a hand edit added or removed supports since the run.
    hand_edited: bool,
}

/// The first hand-placed id is the one after this.
const HAND_IDS_FROM: u64 = 1_000_000;

/// The supports as a removal reads them: each entity's id, the points it
/// passes through, the ids of its parts, and who names whom.
#[derive(Debug, Default)]
struct RemovalIndex {
    ids: Vec<String>,
    /// Per entity, its points in document order (tips, joints, bases).
    points: Vec<Vec<[f64; 3]>>,
    /// Per entity, its own id and its parts' (segments, joints).
    parts: Vec<HashSet<String>>,
    /// Per entity, every string it holds (the ids it names).
    names: Vec<HashSet<String>>,
    /// Who names each string.
    named_by: std::collections::HashMap<String, Vec<usize>>,
    index_of: std::collections::HashMap<String, usize>,
}

impl RemovalIndex {
    fn of(c: &SupportCollections) -> Self {
        let mut index = RemovalIndex::default();
        for (k, (id, value)) in entities(c).into_iter().enumerate() {
            let mut points = Vec::new();
            points_of(&value, &mut points);
            let mut parts = HashSet::new();
            ids_within(&value, &mut parts);
            parts.insert(id.clone());
            let mut names = HashSet::new();
            strings_in(&value, &mut names);
            for name in &names {
                index.named_by.entry(name.clone()).or_default().push(k);
            }
            index.index_of.insert(id.clone(), k);
            index.ids.push(id);
            index.points.push(points);
            index.parts.push(parts);
            index.names.push(names);
        }
        index
    }
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

/// The points near `point` to try, nearest first: lower down the same
/// spot, then around it in rings a tenth of a millimetre apart out to
/// `reach_mm`, each at the same and at lower heights.
fn nudges(point: [f64; 3], reach_mm: f64) -> Vec<[f64; 3]> {
    let [x, y, z] = point;
    let mut out: Vec<[f64; 3]> = NUDGE_DOWN_MM.iter().map(|dz| [x, y, z - dz]).collect();
    // A small margin, or 0.3 / 0.1 rounds down to two rings.
    let rings = (reach_mm / 0.1 + 1e-9).floor() as usize;
    for ring in 1..=rings {
        let r = ring as f64 * 0.1;
        for dz in [0.0, NUDGE_DOWN_MM[0], NUDGE_DOWN_MM[1]] {
            for k in 0..NUDGE_DIRECTIONS {
                let a = k as f64 * std::f64::consts::TAU / NUDGE_DIRECTIONS as f64;
                out.push([x + r * a.cos(), y + r * a.sin(), z - dz]);
            }
        }
    }
    out
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
        recipe: Option<SettingsRecipe>,
        state: SupportState,
        id_prefix: String,
    ) -> Self {
        Self::restored(models, settings, recipe, state, id_prefix, HAND_IDS_FROM)
    }

    /// A session from its parts, with the next hand-placed id counting on
    /// from `next_id`. A saved session only tells hand additions, by their
    /// ids: one with any reads as edited by hand.
    pub(crate) fn restored(
        models: Vec<ModelInput>,
        settings: PipelineSettings,
        recipe: Option<SettingsRecipe>,
        state: SupportState,
        id_prefix: String,
        next_id: u64,
    ) -> Self {
        let bvhs = (0..models.len()).map(|_| OnceLock::new()).collect();
        let sdfs = (0..models.len()).map(|_| SdfSlot::default()).collect();
        DfSupportSession {
            removal_index: Arc::default(),
            models: models.into(),
            settings: Arc::new(settings),
            recipe: recipe.map(Arc::new),
            state,
            bvhs,
            sdfs,
            id_prefix,
            next_id,
            hand_edited: next_id > HAND_IDS_FROM,
        }
    }

    /// The models, the recipe of the settings, the id prefix and the next
    /// hand-placed id: what a saved session keeps besides the state.
    pub(crate) fn parts(&self) -> (&[ModelInput], Option<&SettingsRecipe>, &str, u64) {
        (
            &self.models,
            self.recipe.as_deref(),
            &self.id_prefix,
            self.next_id,
        )
    }

    /// The support state as it now stands.
    pub fn state(&self) -> &SupportState {
        &self.state
    }

    /// Whether supports were added or removed by hand since the run: a run
    /// made again would not keep those edits.
    pub fn hand_edited(&self) -> bool {
        self.hand_edited
    }

    /// How many models the session was made on.
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Whether every support stands on model `index`, so the support mesh
    /// is that model's alone and can move with it as one piece.
    pub fn supports_only_on(&self, index: usize) -> bool {
        let Some(model) = self.models.get(index) else {
            return false;
        };
        bases(&self.state.collections).all(|base| base.model_id == model.id)
    }

    /// The session with model `index` moved by `delta` (mm along X and Y)
    /// and every support that stands on it moved with it (#119): nothing is
    /// placed again. The models' geometry is shared; the moved model's BVH
    /// and distance field build afresh on the next edit, and the others'
    /// carry over.
    pub fn moved_in_plane(&self, index: usize, delta: [f64; 2]) -> Self {
        let mut model = self.models[index].clone();
        model.transform.position = model
            .transform
            .position
            .add(Vec3::new(delta[0], delta[1], 0.0));
        self.taken_along(index, model, &PlaneMotion::shift(delta))
    }

    /// The session with model `index` turned by `angle` radians about the Z
    /// axis through the origin, then moved by `delta` (mm along X and Y), and
    /// every support that stands on it taken along (#153): the model still
    /// stands on the plate the way it did, so its supports still hold it, and
    /// nothing is placed again. `now` is the model where it now stands, as a
    /// run makes it from the scene; it keeps the session's id. The moved
    /// model's BVH and distance field build afresh on the next edit, and the
    /// others' carry over.
    pub fn turned_in_plane(
        &self,
        index: usize,
        mut now: ModelInput,
        angle: f64,
        delta: [f64; 2],
    ) -> Self {
        now.id = self.models[index].id.clone();
        self.taken_along(index, now, &PlaneMotion::turn(angle, delta))
    }

    /// The session with model `index` replaced by `model`, its supports
    /// taken along by `motion`.
    fn taken_along(&self, index: usize, model: ModelInput, motion: &PlaneMotion) -> Self {
        let mut state = self.state.clone();
        move_model_supports(&mut state.collections, &model.id, motion);
        let mut models = self.models.to_vec();
        models[index] = model;
        let bvhs = self
            .bvhs
            .iter()
            .enumerate()
            .map(|(i, bvh)| match bvh.get() {
                Some(bvh) if i != index => OnceLock::from(bvh.clone()),
                _ => OnceLock::new(),
            })
            .collect();
        // Taken, as an edit borrows them: Undo back to this session builds
        // its own again.
        let sdfs = self
            .sdfs
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                let field = (i != index)
                    .then(|| slot.0.lock().ok().and_then(|mut slot| slot.take()))
                    .flatten();
                SdfSlot(std::sync::Mutex::new(field))
            })
            .collect();
        DfSupportSession {
            models: models.into(),
            settings: self.settings.clone(),
            recipe: self.recipe.clone(),
            state,
            bvhs,
            removal_index: Arc::default(),
            sdfs,
            id_prefix: self.id_prefix.clone(),
            next_id: self.next_id,
            hand_edited: self.hand_edited,
        }
    }

    /// One session per model, in model order, each holding only the supports
    /// that stand on its model (#197). DragonFruit keeps each model's
    /// supports to themselves, so nothing is placed again; each session's
    /// raft is drawn from its own roots. The models' geometry, BVHs and
    /// distance fields carry over. Hand-placed ids stay apart: each session
    /// names new supports under its own prefix.
    pub fn split_by_model(&self) -> Vec<Self> {
        if self.models.len() == 1 {
            return vec![self.clone()];
        }
        (0..self.models.len())
            .map(|i| {
                let model = self.models[i].clone();
                let collections = model_collections(&self.state.collections, &model.id);
                let bvh = self.bvhs[i]
                    .get()
                    .map_or_else(OnceLock::new, |bvh| OnceLock::from(bvh.clone()));
                let sdf = self.sdfs[i].0.lock().ok().and_then(|mut slot| slot.take());
                DfSupportSession {
                    models: vec![model].into(),
                    settings: self.settings.clone(),
                    recipe: self.recipe.clone(),
                    state: SupportState {
                        collections,
                        ..SupportState::default()
                    },
                    bvhs: vec![bvh].into(),
                    removal_index: Arc::default(),
                    sdfs: vec![SdfSlot(std::sync::Mutex::new(sdf))].into(),
                    id_prefix: format!("{}.{i}", self.id_prefix),
                    next_id: self.next_id,
                    hand_edited: self.hand_edited,
                }
            })
            .collect()
    }

    /// Whether the session holds no support at all.
    pub fn is_empty(&self) -> bool {
        bases(&self.state.collections).next().is_none()
    }

    /// How far `point` (world mm) is from the nearest model surface an Add
    /// click there would hold; `None` beyond reach.
    pub fn add_distance(&self, point: [f64; 3]) -> Option<f64> {
        let (_, tip, _) = self.contact(point)?;
        Some(dist(point, [tip.x, tip.y, tip.z]))
    }

    /// How far `point` (world mm) is from the support a Remove click there
    /// would take; `None` beyond reach.
    pub fn remove_distance(&self, point: [f64; 3]) -> Option<f64> {
        self.nearest_support(point)
            .map(|(d, _)| d)
            .filter(|d| *d <= REMOVE_REACH_MM)
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
        // A hand-placed support hangs from a support up to
        // MANUAL_ATTACH_REACH_MM away rather than standing on its own: the
        // auto-placer's fan reach (8 mm) is tuned for a whole run, where a
        // lone trunk is cheap; by hand, attaching is what the person wants.
        // DragonFruit's angle, collision and per-host limits still apply.
        let mut settings = (*self.settings).clone();
        let fan = &mut settings.supports.auto_support.leaf_fan_radius_mm;
        *fan = fan.max(MANUAL_ATTACH_REACH_MM);
        let settings = Arc::new(settings);
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
        self.removal_index = Arc::default();
        if let Ok(mut slot) = self.sdfs[i].0.lock() {
            *slot = Some(sdf.into_inner());
        }
        if outcome.kind.is_none() {
            let why = outcome
                .rejected_reason
                .map_or_else(|| "rejected".to_owned(), |r| format!("{r:?}"));
            return Err(EditError::NoRoute(why));
        }
        self.hand_edited = true;
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

    /// Adds a support at `point` (world mm) as [`Self::add_trunk_at`] does
    /// or, when nothing can hold that very point, at the nearest point the
    /// engine can hold on a face that looks down, whose tip lands within
    /// `reach_mm` of the one `point` gives: up to 0.4 mm lower and `reach_mm`
    /// to the side. For a
    /// point a check picked rather than a click (the Review card's islands):
    /// the route from one tip can graze the model where a tip a step away
    /// clears it. Returns the point used; the error is the one at `point`.
    pub fn add_trunk_near(
        &mut self,
        point: [f64; 3],
        reach_mm: f64,
    ) -> Result<[f64; 3], EditError> {
        let Some((_, aimed, normal)) = self.contact(point) else {
            return Err(EditError::NothingThere);
        };
        // A face that looks up holds nothing above it: only the points next
        // to it are tried.
        let mut first = EditError::NoRoute("the nearest face looks up".to_owned());
        if normal.z < 0.0 {
            match self.place(point) {
                Ok(_) => return Ok(point),
                Err(err @ EditError::NoRoute(_)) => first = err,
                Err(other) => return Err(other),
            }
        }
        // A tip that lands farther than `reach_mm` from the one `point` gives
        // would hold some other surface than the one asked for.
        for q in nudges(point, reach_mm) {
            let holds = self.contact(q).is_some_and(|(_, tip, n)| {
                n.z < 0.0 && dist([tip.x, tip.y, tip.z], [aimed.x, aimed.y, aimed.z]) <= reach_mm
            });
            if holds && self.place(q).is_ok() {
                return Ok(q);
            }
        }
        Err(first)
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
    fn removal_index(&self) -> Arc<RemovalIndex> {
        self.removal_index
            .get_or_init(|| Arc::new(RemovalIndex::of(&self.state.collections)))
            .clone()
    }

    /// The support nearest `point`, knots left out: its distance and its
    /// place in the removal index.
    fn nearest_support(&self, point: [f64; 3]) -> Option<(f64, usize)> {
        let index = self.removal_index();
        let c = &self.state.collections;
        let mut best: Option<(f64, usize)> = None;
        for (k, pts) in index.points.iter().enumerate() {
            if c.knots.contains_key(&index.ids[k]) {
                continue;
            }
            let d = pts
                .windows(2)
                .map(|w| segment_dist(point, w[0], w[1]))
                .chain(pts.iter().map(|q| dist(point, *q)))
                .fold(f64::INFINITY, f64::min);
            if d < best.map_or(f64::INFINITY, |b| b.0) {
                best = Some((d, k));
            }
        }
        best
    }

    fn removal(&self, point: [f64; 3]) -> Result<HashSet<String>, EditError> {
        let index = self.removal_index();
        let c = &self.state.collections;
        let Some((d, start)) = self.nearest_support(point) else {
            return Err(EditError::NothingThere);
        };
        if d > REMOVE_REACH_MM {
            return Err(EditError::NothingThere);
        }
        // The cascade: what names the removed entity or any of its parts (a
        // knot names the shaft segment it sits on) goes with it.
        let n = index.ids.len();
        let mut gone = vec![false; n];
        gone[start] = true;
        let mut queue = vec![start];
        while let Some(g) = queue.pop() {
            for part in &index.parts[g] {
                for &k in index.named_by.get(part).into_iter().flatten() {
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
        let mut orphans = Vec::new();
        for g in (0..n).filter(|&g| gone[g]) {
            for name in &index.names[g] {
                let Some(&k) = index.index_of.get(name) else {
                    continue;
                };
                let owned = c.roots.contains_key(name) || c.knots.contains_key(name);
                if gone[k] || !owned {
                    continue;
                }
                // Its own record names it: only others count.
                let still_used = index
                    .named_by
                    .get(name)
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
        Ok((0..n)
            .filter(|&k| gone[k])
            .map(|k| index.ids[k].clone())
            .collect())
    }

    /// Builds the removal index now (a worker does it right after an edit,
    /// so the first removal preview on the new state is quick).
    pub fn warm_removal(&self) {
        self.removal_index();
    }

    /// Removes the support nearest `point` (world mm) and everything that
    /// hangs from it. Returns how many entities went.
    pub fn remove_near(&mut self, point: [f64; 3]) -> Result<usize, EditError> {
        let gone = self.removal(point)?;
        self.state.collections = filtered(&self.state.collections, |id| !gone.contains(id));
        self.removal_index = Arc::default();
        self.hand_edited = true;
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
    fn nudges_go_down_first_then_out_in_rings_to_the_reach() {
        let points = nudges([1.0, 2.0, 3.0], 0.3);
        // Four steps down, then three rings of eight at three heights.
        assert_eq!(points.len(), 4 + 3 * 3 * 8);
        assert_eq!(points[0], [1.0, 2.0, 2.95]);
        let far = |p: &[f64; 3]| (p[0] - 1.0).hypot(p[1] - 2.0);
        assert!(points[..4].iter().all(|p| far(p) == 0.0));
        let rings: Vec<f64> = points[4..].iter().map(far).collect();
        assert!(
            rings.windows(2).all(|w| w[0] <= w[1] + 1e-9),
            "nearest first"
        );
        assert!(rings.iter().all(|r| *r <= 0.3 + 1e-9));
        assert!(points.iter().all(|p| p[2] <= 3.0));
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

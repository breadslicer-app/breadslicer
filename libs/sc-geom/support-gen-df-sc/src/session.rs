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
use support_gen_df::supports::auto_bracing::auto_brace::brace_new_shaft;
use support_gen_df::supports::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use support_gen_df::supports::auto_support::auto_place::fan::{
    FanArgs, FanHostOrder, FanLeafRefusal, FanLeafResult, add_fanned, fan_leaf_to_host,
};
use support_gen_df::supports::auto_support::auto_place::helpers::{
    FanShaftPoint, PlanCtx, PlanMesh, collect_fan_shaft_points, host_segment_span,
    member_max_angle_from_vertical_deg,
};
use support_gen_df::supports::auto_support::auto_place::place::{
    PlaceArgs, RejectReason, place_one_candidate,
};
use support_gen_df::supports::auto_support::parameter_sizing::SizingBands;
use support_gen_df::supports::auto_support::types::{CandidatePoint, CandidateSource};
use support_gen_df::supports::auto_support::zone_rules::ZoneRules;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::rafts::crenelated::raft_types::RaftBottomMode;
use support_gen_df::supports::support_type_registry::{
    SupportOriginId, SupportTypeId, near_plate_origin,
};
use support_gen_df::supports::support_types::trunk::trunk_builder::{
    TrunkBuildInput, build_trunk_data,
};
use support_gen_df::supports::types::LimitationCode;
use support_gen_df::supports::types::{SupportCollections, SupportState};
use support_gen_df::three::{Matrix4, Vec3};
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};

use crate::document::ModelPrint;
use crate::shift::{PlaneMotion, bases, model_collections, move_model_supports};
use crate::{DF_SUPPORT_MESH_NAME, SettingsRecipe, weld_parts};

/// How far from the model surface a click may land and still place a support.
const ADD_REACH_MM: f64 = 2.0;
/// How far a hand-placed support reaches for a support to hang from.
const MANUAL_ATTACH_REACH_MM: f64 = 15.0;
/// How far from vertical a hand-placed support may lean to hang from
/// another (#3). A run holds its members to 30° (DragonFruit's branch-angle
/// rule, 60° from flat), tuned for a whole plate where a lone trunk is cheap;
/// by hand that built a trunk of its own right next to one in reach.
const MANUAL_MAX_ANGLE_DEG: f64 = 45.0;
/// How far a forced attach (Shift+click) reaches for the nearest support.
const FORCED_ATTACH_REACH_MM: f64 = 30.0;
/// How far from vertical a forced attach may lean.
const FORCED_MAX_ANGLE_DEG: f64 = 60.0;
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
    /// Per model, what tells it apart in a document, made on the first save
    /// or comparison (a pass over every vertex) and shared by every copy.
    prints: Arc<[OnceLock<ModelPrint>]>,
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
pub(crate) const HAND_IDS_FROM: u64 = 1_000_000;

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
    /// A forced attach found no support to hang from, for this reason.
    CannotAttach(AddNote),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NothingThere => write!(f, "nothing within reach of the click"),
            EditError::NoRoute(why) => {
                write!(f, "no way to support that point ({why})")
            }
            EditError::Tessellation(why) => write!(f, "the supports could not be drawn ({why})"),
            EditError::CannotAttach(why) => write!(f, "no support to hang it from ({why:?})"),
        }
    }
}

/// How a support is added by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct AddOptions {
    /// Hang it from the nearest support it can reach without crossing the
    /// model, or place nothing (Shift+click), instead of the auto-placer's
    /// choice between hanging and a trunk of its own.
    pub attach: bool,
    /// The size it takes, by id (`"small"`): one of the run's zone sizes,
    /// else one of [`crate::zone_presets::support_sizes`]. `None` takes the
    /// size the run's preset gives a support placed by hand.
    pub size: Option<String>,
}

/// Why a support added by hand does not hang from another, or cannot be
/// placed at all: what the preview tells in one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddNote {
    /// No support close enough to hang from.
    TooFar,
    /// The supports in reach are not below the tip.
    NoneBelow,
    /// Hanging from the supports in reach would lean too far.
    TooSteep,
    /// The way to every support in reach goes through the model.
    HitsModel,
    /// The way to the supports in reach crosses another support.
    CrossesSupport,
    /// The supports in reach carry all the members they may.
    HostsFull,
    /// The surface there is too steep for a tip.
    SteepSurface,
}

impl AddNote {
    fn of_fan(refusal: FanLeafRefusal) -> Self {
        match refusal {
            FanLeafRefusal::NoHost => AddNote::TooFar,
            FanLeafRefusal::SameZ => AddNote::NoneBelow,
            FanLeafRefusal::Angle => AddNote::TooSteep,
            FanLeafRefusal::Blocked | FanLeafRefusal::Build => AddNote::HitsModel,
            FanLeafRefusal::Cross => AddNote::CrossesSupport,
            FanLeafRefusal::Capacity => AddNote::HostsFull,
        }
    }

    /// Why nothing could hold a point, when the reason is one to tell.
    fn of_rejection(reason: RejectReason) -> Option<Self> {
        match reason {
            RejectReason::Trunk(LimitationCode::CollisionWithModel)
            | RejectReason::GridRejectCollision => Some(AddNote::HitsModel),
            RejectReason::Trunk(LimitationCode::AngleTooSteep) | RejectReason::SideWallContact => {
                Some(AddNote::SteepSurface)
            }
            _ => None,
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
    /// For an addition that goes through: it hangs from another support.
    pub attached: bool,
    /// For an addition: why it stands on its own, or why it cannot be
    /// placed, when there is a reason to tell.
    pub note: Option<AddNote>,
}

/// What [`DfSupportSession::place`] added.
struct Placed {
    added: HashSet<String>,
    attached: bool,
    note: Option<AddNote>,
}

/// The settings a support added by hand is made with: the run's, with the
/// limits on hanging from a support eased (and eased further for a forced
/// attach) and the size `how` names. Returns that size's index among the
/// zone sizes, `None` for the run's own.
fn hand_settings(settings: &mut PipelineSettings, how: &AddOptions) -> Option<usize> {
    let max_angle = if how.attach {
        FORCED_MAX_ANGLE_DEG
    } else {
        MANUAL_MAX_ANGLE_DEG
    };
    let supports = &mut settings.supports;
    let auto = &mut supports.auto_support;
    // The auto-placer's fan reach (8 mm) is tuned for a whole run; by hand,
    // hanging is what the person wants. Collisions are still checked.
    auto.leaf_fan_radius_mm = auto.leaf_fan_radius_mm.max(MANUAL_ATTACH_REACH_MM);
    auto.leaf_fan_max_angle_deg = auto.leaf_fan_max_angle_deg.max(max_angle);
    // No cap on the members a support carries (0 is none).
    auto.max_attachments_per_trunk = 0.0;
    supports.grid.min_branch_angle_deg = supports.grid.min_branch_angle_deg.min(90.0 - max_angle);
    let id = how.size.as_deref()?;
    if let Some(i) = auto.zone_rules.as_ref().and_then(|r| r.size_index(id)) {
        return Some(i);
    }
    let size = crate::zone_presets::support_sizes()
        .into_iter()
        .find(|s| s.id == id)?;
    // Rules holding only this size draw it exactly, as a zone preset does.
    let rules = auto.zone_rules.get_or_insert_with(ZoneRules::default);
    rules.sizes.push(size);
    Some(rules.sizes.len() - 1)
}

/// How far apart the bottom segment of a trunk is sampled for a support
/// added by hand to hang from, and how many samples at most.
const HAND_POOL_STEP_MM: f64 = 1.0;
const HAND_POOL_SAMPLES: f64 = 64.0;

/// The shaft points a support added by hand may hang from: the fan's (every
/// trunk segment between two joints) and, unlike a run's, each trunk's
/// bottom segment, from its root up. A straight trunk is a bottom segment
/// and a short top one, so a run's fan sees only its last few millimetres
/// and a click beside it found nothing low enough to reach.
fn hand_pool(state: &SupportState) -> Vec<FanShaftPoint> {
    let mut pool = collect_fan_shaft_points(state);
    let near_plate = near_plate_origin();
    for (id, trunk) in &state.collections.trunks {
        if trunk.base.origin == Some(near_plate) {
            continue;
        }
        for seg in trunk.segments.iter().filter(|s| s.bottom_joint.is_none()) {
            let Some((start, end)) = host_segment_span(state, trunk, seg) else {
                continue;
            };
            let span = dist([start.x, start.y, start.z], [end.x, end.y, end.z]);
            let steps = (span / HAND_POOL_STEP_MM)
                .ceil()
                .clamp(1.0, HAND_POOL_SAMPLES);
            for k in 0..=steps as u32 {
                let t = f64::from(k) / steps;
                pool.push(FanShaftPoint {
                    host_type_id: SupportTypeId::Trunk,
                    host_id: id.clone(),
                    segment_id: Some(seg.id.clone()),
                    t: Some(t),
                    pos: Vec3::new(
                        start.x + (end.x - start.x) * t,
                        start.y + (end.y - start.y) * t,
                        start.z + (end.z - start.z) * t,
                    ),
                    diameter: seg.diameter,
                });
            }
        }
    }
    pool
}

/// Hangs `candidate` from a support of its model within `reach_mm` it can
/// reach without crossing the model or another support, leaning no further
/// from vertical than the live settings let a member: DragonFruit's fan on
/// [`hand_pool`], the steepest host first or the nearest. Adds the knot and
/// the member to `state`, or says why none was found.
fn hang(
    candidate: &CandidatePoint,
    state: &mut SupportState,
    ctx: &PlanCtx<'_>,
    reach_mm: f64,
    order: FanHostOrder,
) -> Result<(), AddNote> {
    let live = ctx.live();
    let max_angle_deg = live
        .auto_support
        .leaf_fan_max_angle_deg
        .min(member_max_angle_from_vertical_deg(live));
    let pool = hand_pool(state);
    let fan = fan_leaf_to_host(
        &FanArgs {
            target: candidate.tip_pos,
            model_id: &candidate.model_id,
            shaft_points: &pool,
            grid_host_ids: &HashSet::new(),
            knot_id_prefix: &format!("auto-fan-{}", candidate.id),
            fan_radius_mm: reach_mm,
            grid_host_fan_radius_mm: reach_mm,
            max_angle_deg,
            max_attachments: 0.0,
            draft: state,
            origin: Some(SupportOriginId::Overhang),
            host_order: order,
            rescue_span_override_mm: None,
        },
        ctx,
    );
    match fan {
        FanLeafResult::Ok {
            kind,
            parent_knot,
            member,
            ..
        } => {
            add_fanned(state, kind, *parent_knot, *member);
            Ok(())
        }
        FanLeafResult::Refused { reason, .. } => Err(AddNote::of_fan(reason)),
    }
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

/// The collections with only the entities `keep` says, built from those
/// alone: [`filtered`]'s result without copying what it would drop.
fn picked(c: &SupportCollections, keep: impl Fn(&String) -> bool) -> SupportCollections {
    fn pick<T: Clone>(
        map: &indexmap::IndexMap<String, T>,
        keep: &impl Fn(&String) -> bool,
    ) -> indexmap::IndexMap<String, T> {
        map.iter()
            .filter(|(id, _)| keep(id))
            .map(|(id, e)| (id.clone(), e.clone()))
            .collect()
    }
    SupportCollections {
        roots: pick(&c.roots, &keep),
        trunks: pick(&c.trunks, &keep),
        branches: pick(&c.branches, &keep),
        leaves: pick(&c.leaves, &keep),
        twigs: pick(&c.twigs, &keep),
        sticks: pick(&c.sticks, &keep),
        braces: pick(&c.braces, &keep),
        stumps: pick(&c.stumps, &keep),
        kickstands: pick(&c.kickstands, &keep),
        knots: pick(&c.knots, &keep),
    }
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
        Self::restored(
            models,
            settings,
            recipe,
            state,
            id_prefix,
            HAND_IDS_FROM,
            false,
        )
    }

    /// A session from its parts, with the next hand-placed id counting on
    /// from `next_id`.
    pub(crate) fn restored(
        models: Vec<ModelInput>,
        settings: PipelineSettings,
        recipe: Option<SettingsRecipe>,
        state: SupportState,
        id_prefix: String,
        next_id: u64,
        hand_edited: bool,
    ) -> Self {
        let bvhs = (0..models.len()).map(|_| OnceLock::new()).collect();
        let prints = (0..models.len()).map(|_| OnceLock::new()).collect();
        let sdfs = (0..models.len()).map(|_| SdfSlot::default()).collect();
        DfSupportSession {
            removal_index: Arc::default(),
            models: models.into(),
            settings: Arc::new(settings),
            recipe: recipe.map(Arc::new),
            state,
            bvhs,
            prints,
            sdfs,
            id_prefix,
            next_id,
            hand_edited,
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

    /// What tells each model apart in a document, in model order.
    pub(crate) fn model_prints(&self) -> Vec<ModelPrint> {
        self.models
            .iter()
            .zip(self.prints.iter())
            .map(|(model, print)| print.get_or_init(|| ModelPrint::of(model)).clone())
            .collect()
    }

    /// The support state as it now stands.
    pub fn state(&self) -> &SupportState {
        &self.state
    }

    /// The options the run was made with; `None` for a run given explicit
    /// settings.
    pub fn options(&self) -> Option<&crate::DfSupportOptions> {
        self.recipe.as_deref().map(|r| &r.options)
    }

    /// Whether supports were added or removed by hand since the run: a run
    /// made again would not keep those edits.
    pub fn hand_edited(&self) -> bool {
        self.hand_edited
    }

    /// The zone size (#360) a support added by hand takes when no size is
    /// asked for: the one the run's preset gives it. `None` for a run whose
    /// preset has no sizes of its own.
    pub fn preset_hand_size(&self) -> Option<&str> {
        let rules = self.settings.supports.auto_support.zone_rules.as_ref()?;
        rules
            .default_size_index()
            .map(|i| rules.sizes[i].id.as_str())
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
        let prints = self
            .prints
            .iter()
            .enumerate()
            .map(|(i, print)| match print.get() {
                Some(print) if i != index => OnceLock::from(print.clone()),
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
            prints,
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
                let print = self.prints[i]
                    .get()
                    .map_or_else(OnceLock::new, |print| OnceLock::from(print.clone()));
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
                    prints: vec![print].into(),
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

    /// Hands the run's BVHs to the session, one per model in model order:
    /// the plans built them on the same world soup the session would.
    pub(crate) fn adopt_bvhs(&self, bvhs: Vec<Arc<MeshBvh>>) {
        for (slot, bvh) in self.bvhs.iter().zip(bvhs) {
            // One already built stays: it is the same tree.
            let _ = slot.set(bvh);
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

    /// Places a support whose tip touches the model at `point` (world mm),
    /// made as `how` says. Returns what it added; a refusal comes with the
    /// reason to tell, when there is one.
    fn place(
        &mut self,
        point: [f64; 3],
        how: &AddOptions,
    ) -> Result<Placed, (EditError, Option<AddNote>)> {
        let Some((i, tip, normal)) = self.contact(point) else {
            return Err((EditError::NothingThere, None));
        };
        let bvh = self.bvh(i);
        let mut settings = (*self.settings).clone();
        let zone = hand_settings(&mut settings, how);
        let model_id = self.models[i].id.clone();
        let before = all_ids(&self.state.collections);
        let pooled = self.sdfs[i].0.lock().ok().and_then(|mut slot| slot.take());
        let sdf = RefCell::new(pooled.unwrap_or_else(|| {
            SdfCache::new(bvh.clone(), Matrix4::IDENTITY, Some(settings.sdf_cell_mm))
        }));
        let bands = SizingBands::factory()
            .with_zone_sizes(settings.supports.auto_support.zone_rules.as_ref());
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
            ctx.set_zone(zone);
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
                zone_size: zone,
            };
            if how.attach {
                hang(
                    &candidate,
                    &mut state,
                    &ctx,
                    FORCED_ATTACH_REACH_MM,
                    FanHostOrder::Nearest,
                )
                .map(|()| None)
                .map_err(|note| (EditError::CannotAttach(note), Some(note)))
            } else {
                let reach = settings.supports.auto_support.leaf_fan_radius_mm;
                match hang(&candidate, &mut state, &ctx, reach, FanHostOrder::Steepest) {
                    Ok(()) => Ok(None),
                    Err(why_not) => {
                        // DragonFruit's placement without its own fan (no
                        // grid hosts skips it): a merge, its own trunk, a
                        // bridge from the model.
                        let outcome = place_one_candidate(
                            &candidate,
                            &mut state,
                            &PlaceArgs {
                                grid_host_ids: None,
                                sizing_ctx: None,
                            },
                            &ctx,
                            false,
                        );
                        match (outcome.kind, outcome.rejected_reason) {
                            (Some(_), _) => Ok(Some(why_not)),
                            (None, reason) => {
                                let why = reason.map_or_else(
                                    || "rejected".to_owned(),
                                    |r| r.as_str().to_owned(),
                                );
                                Err((
                                    EditError::NoRoute(why),
                                    reason.and_then(AddNote::of_rejection),
                                ))
                            }
                        }
                    }
                }
            }
        };
        self.state = state;
        self.removal_index = Arc::default();
        if let Ok(mut slot) = self.sdfs[i].0.lock() {
            *slot = Some(sdf.into_inner());
        }
        let note = outcome?;
        self.hand_edited = true;
        let mut added: HashSet<String> = all_ids(&self.state.collections)
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        // A member hangs from a knot on its host; one standing on its own
        // brings no knot.
        let attached = added
            .iter()
            .any(|id| self.state.collections.knots.contains_key(id));
        // A trunk of its own is braced to its neighbours as a run braces its
        // trunks, when the run braces at all (#3).
        let trunk = (!attached)
            .then(|| {
                added
                    .iter()
                    .find(|id| self.state.collections.trunks.contains_key(*id))
                    .cloned()
            })
            .flatten();
        if let Some(trunk) = trunk
            && !settings.supports.auto_support.debug_skip_auto_bracing
        {
            let meshes: BraceMeshes<'_> = [(
                self.models[i].id.clone(),
                BraceMesh {
                    bvh: &bvh,
                    matrix: Matrix4::IDENTITY,
                },
            )]
            .into();
            let braced = brace_new_shaft(
                &mut self.state,
                &trunk,
                &settings.supports.auto_bracing,
                &settings.supports,
                &meshes,
            );
            if braced > 0 {
                added = all_ids(&self.state.collections)
                    .into_iter()
                    .filter(|id| !before.contains(id))
                    .collect();
            }
        }
        Ok(Placed {
            added,
            attached,
            note: (!attached).then_some(note).flatten(),
        })
    }

    /// Adds a support whose tip touches the model at `point` (world mm), the
    /// way DragonFruit places one contact.
    pub fn add_trunk_at(&mut self, point: [f64; 3]) -> Result<(), EditError> {
        self.add_at(point, &AddOptions::default())
    }

    /// [`Self::add_trunk_at`], made as `how` says.
    pub fn add_at(&mut self, point: [f64; 3], how: &AddOptions) -> Result<(), EditError> {
        self.place(point, how).map(|_| ()).map_err(|(err, _)| err)
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
        let how = AddOptions::default();
        if normal.z < 0.0 {
            match self.place(point, &how) {
                Ok(_) => return Ok(point),
                Err((err @ EditError::NoRoute(_), _)) => first = err,
                Err((other, _)) => return Err(other),
            }
        }
        // A tip that lands farther than `reach_mm` from the one `point` gives
        // would hold some other surface than the one asked for.
        for q in nudges(point, reach_mm) {
            let holds = self.contact(q).is_some_and(|(_, tip, n)| {
                n.z < 0.0 && dist([tip.x, tip.y, tip.z], [aimed.x, aimed.y, aimed.z]) <= reach_mm
            });
            if holds && self.place(q, &how).is_ok() {
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
        self.preview_add_with(point, &AddOptions::default())
    }

    /// [`Self::preview_add`], made as `how` says.
    pub fn preview_add_with(
        &self,
        point: [f64; 3],
        how: &AddOptions,
    ) -> Result<(EditPreview, Option<Self>), EditError> {
        let mut copy = self.clone();
        match copy.place(point, how) {
            Ok(placed) => {
                let preview = EditPreview {
                    mesh: copy.mesh_of(|id| placed.added.contains(id))?,
                    valid: true,
                    attached: placed.attached,
                    note: placed.note,
                };
                Ok((preview, Some(copy)))
            }
            Err((EditError::NoRoute(_) | EditError::CannotAttach(_), note)) => {
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
                    attached: false,
                    note,
                };
                Ok((preview, None))
            }
            Err((other, _)) => Err(other),
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
            attached: false,
            note: None,
        })
    }

    /// The entities `keep` names, tessellated without a raft. A preview
    /// draws a few pieces of a big set on every hover: only those are
    /// copied, and the models go without their geometry, which only a raft
    /// reads (a 1.9M-face model cost ~8 ms a preview, #3).
    fn mesh_of(&self, keep: impl Fn(&String) -> bool) -> Result<IndexedMesh, EditError> {
        let state = SupportState {
            collections: picked(&self.state.collections, keep),
            ..SupportState::default()
        };
        let mut settings = (*self.settings).clone();
        settings.rafts.global.bottom_mode = RaftBottomMode::Off;
        settings.rafts.per_model.clear();
        let models: Vec<ModelInput> = self
            .models
            .iter()
            .map(|m| ModelInput {
                id: m.id.clone(),
                geometry: Arc::default(),
                transform: m.transform,
                visible: m.visible,
            })
            .collect();
        let sink = support_soup(&state, &models, &settings)
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

    /// A slab 50 × 10 × 2 mm whose underside is `z` mm above the plate, run
    /// with `request`, its supports then cleared: the hand tools start from
    /// nothing.
    fn bare_slab(z: f64, request: crate::DfSupportRequest) -> DfSupportSession {
        let mut session = crate::start_df_supports(&slab_scene(z), request)
            .unwrap()
            .join()
            .unwrap()
            .sets
            .remove(0)
            .session;
        session.state = SupportState::default();
        session.removal_index = Arc::default();
        session
    }

    /// A slab 50 × 10 × 2 mm whose underside is `z` mm above the plate.
    fn slab_scene(z: f64) -> sc_scene::Scene {
        use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
        use sc_mesh_core::{IndexedTriangle, MeshMetadata, Vertex};
        use sc_scene::{LeafPayload, Scene};
        let v = |x: f32, y: f32, z: f32| Vertex::new([x * 50.0, y * 10.0, z * 2.0]);
        let vertices = vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(1.0, 1.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(1.0, 0.0, 1.0),
            v(1.0, 1.0, 1.0),
            v(0.0, 1.0, 1.0),
        ];
        let f = |a, b, c| IndexedTriangle::from_raw_indices([a, b, c]);
        let faces = vec![
            f(0, 2, 1),
            f(0, 3, 2),
            f(4, 5, 6),
            f(4, 6, 7),
            f(0, 1, 5),
            f(0, 5, 4),
            f(1, 2, 6),
            f(1, 6, 5),
            f(2, 3, 7),
            f(2, 7, 6),
            f(3, 0, 4),
            f(3, 4, 7),
        ];
        let mesh = IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces);
        let mut scene = Scene::new();
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, z])),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        scene.add_root(leaf).unwrap();
        scene
    }

    /// A scene of one model made of boxes, each `(min, max)` in world mm.
    fn boxes_scene(boxes: &[([f32; 3], [f32; 3])]) -> sc_scene::Scene {
        use sc_mesh_core::{IndexedTriangle, MeshMetadata, Vertex};
        use sc_scene::{LeafPayload, Scene};
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        for (lo, hi) in boxes {
            let o = vertices.len() as u32;
            for k in 0..8u32 {
                let pick = |bit: u32, axis: usize| if k & bit == 0 { lo[axis] } else { hi[axis] };
                vertices.push(Vertex::new([pick(1, 0), pick(2, 1), pick(4, 2)]));
            }
            // Corners by bits x = 1, y = 2, z = 4; each side wound outward.
            for [a, b, c] in [
                [0, 2, 1],
                [1, 2, 3],
                [4, 5, 6],
                [5, 7, 6],
                [0, 1, 4],
                [1, 5, 4],
                [2, 6, 3],
                [3, 6, 7],
                [0, 4, 2],
                [2, 4, 6],
                [1, 3, 5],
                [3, 7, 5],
            ] {
                faces.push(IndexedTriangle::from_raw_indices([o + a, o + b, o + c]));
            }
        }
        let mesh = IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces);
        let mut scene = Scene::new();
        let leaf = scene
            .add_leaf(
                None,
                sc_mesh_core::transform::Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        scene.add_root(leaf).unwrap();
        scene
    }

    /// A slab 50 × 10 mm with its underside 25 mm up and a wall hanging from
    /// it to 12 mm between x 9 and 11, run with `request`, its supports then
    /// cleared, and a trunk of its own under `[5, 5]`.
    fn slab_with_a_wall(request: crate::DfSupportRequest) -> DfSupportSession {
        let scene = boxes_scene(&[
            ([0.0, 0.0, 25.0], [50.0, 10.0, 27.0]),
            ([9.0, 0.0, 12.0], [11.0, 10.0, 25.0]),
        ]);
        let mut session = crate::start_df_supports(&scene, request)
            .unwrap()
            .join()
            .unwrap()
            .sets
            .remove(0)
            .session;
        session.state = SupportState::default();
        session.removal_index = Arc::default();
        session.add_trunk_at([5.0, 5.0, 25.0]).expect("a trunk");
        assert!(session.state.collections.braces.is_empty());
        session
    }

    #[test]
    fn a_trunk_of_its_own_is_braced_to_its_neighbour_clear_of_the_model() {
        let session = slab_with_a_wall(crate::DfSupportRequest::default());
        let (preview, edited) = session
            .preview_add_with([14.0, 5.0, 25.0], &AddOptions::default())
            .expect("a preview");
        assert!(
            preview.valid && !preview.attached,
            "{:?}",
            (preview.valid, preview.attached, preview.note)
        );
        let edited = edited.expect("the session the click leaves");
        let c = &edited.state.collections;
        assert_eq!(c.trunks.len(), 2);
        assert!(!c.braces.is_empty(), "braced to the trunk beside it");
        // Each brace joins the two trunks, and none goes through the wall.
        let owner = |knot: &str| {
            let shaft = &c.knots[knot].parent_shaft_id;
            c.trunks
                .iter()
                .find(|(_, t)| t.segments.iter().any(|s| &s.id == shaft))
                .map(|(id, _)| id.clone())
        };
        for brace in c.braces.values() {
            let (a, b) = (owner(&brace.start_knot_id), owner(&brace.end_knot_id));
            assert!(a.is_some() && b.is_some() && a != b);
            let (p, q) = (
                c.knots[&brace.start_knot_id].pos,
                c.knots[&brace.end_knot_id].pos,
            );
            for k in 0..=20 {
                let t = f64::from(k) / 20.0;
                let (x, z) = (p.x + (q.x - p.x) * t, p.z + (q.z - p.z) * t);
                assert!(
                    !((8.6..=11.4).contains(&x) && z >= 11.6),
                    "a brace through the wall at x {x:.2}, z {z:.2}"
                );
            }
        }
        // The braces go with it: a click on it takes them all.
        let mut removed = edited.clone();
        removed
            .remove_near([14.0, 5.0, 24.9])
            .expect("the new trunk");
        assert_eq!(removed.state.collections, session.state.collections);
    }

    #[test]
    fn a_new_trunk_is_braced_by_the_preset_or_not_at_all() {
        let braces = |request: crate::DfSupportRequest| {
            let mut session = slab_with_a_wall(request);
            session.add_trunk_at([14.0, 5.0, 25.0]).expect("a trunk");
            session
                .state
                .collections
                .braces
                .values()
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut off = crate::DfSupportRequest::default();
        off.options.bracing = false;
        assert!(braces(off).is_empty(), "no bracing, no braces");
        let mut minis = crate::DfSupportRequest::default();
        minis.options.zones = Some(crate::zone_presets::minis_30());
        let zoned = braces(minis);
        assert!(!zoned.is_empty());
        let default = braces(crate::DfSupportRequest::default());
        assert_ne!(
            zoned.iter().map(|b| b.profile).collect::<Vec<_>>(),
            default.iter().map(|b| b.profile).collect::<Vec<_>>(),
            "the preset's own brace"
        );
    }

    /// The slab with one trunk of its own under `[5, 5]`.
    fn slab_with_a_trunk() -> DfSupportSession {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        session.add_trunk_at([5.0, 5.0, 12.0]).expect("a trunk");
        assert!(session.state.collections.knots.is_empty(), "on its own");
        assert_eq!(session.state.collections.trunks.len(), 1);
        session
    }

    /// The knots `edited` holds that `before` does not.
    fn new_knots<'a>(
        before: &DfSupportSession,
        edited: &'a DfSupportSession,
    ) -> Vec<&'a support_gen_df::supports::types::Knot> {
        edited
            .state
            .collections
            .knots
            .iter()
            .filter(|(id, _)| !before.state.collections.knots.contains_key(*id))
            .map(|(_, knot)| knot)
            .collect()
    }

    #[test]
    fn a_click_beside_a_trunk_hangs_from_it() {
        let session = slab_with_a_trunk();
        let (preview, edited) = session
            .preview_add_with([11.0, 5.0, 12.0], &AddOptions::default())
            .expect("a preview");
        assert!(
            preview.valid && preview.attached,
            "{:?}",
            (preview.valid, preview.attached, preview.note)
        );
        assert_eq!(preview.note, None);
        let edited = edited.expect("the session the click leaves");
        assert_eq!(
            edited.state.collections.roots.len(),
            1,
            "no root of its own"
        );
        // It hangs low on the trunk's bottom segment, which a run's fan never
        // offers: a run built a trunk of its own here.
        let knots = new_knots(&session, &edited);
        assert_eq!(knots.len(), 1);
        let trunk = session.state.collections.trunks.values().next().unwrap();
        let bottom = trunk
            .segments
            .iter()
            .find(|s| s.bottom_joint.is_none())
            .expect("a bottom segment");
        assert_eq!(knots[0].parent_shaft_id, bottom.id);
    }

    #[test]
    fn shift_click_hangs_from_the_nearest_support_or_says_why() {
        let session = slab_with_a_trunk();
        let attach = AddOptions {
            attach: true,
            size: None,
        };
        // Too far to the side for a plain click: a trunk of its own, and why.
        let tip = [17.0, 5.0, 12.0];
        let (plain, _) = session
            .preview_add_with(tip, &AddOptions::default())
            .expect("a preview");
        assert!(
            plain.valid && !plain.attached,
            "{:?}",
            (plain.valid, plain.attached, plain.note)
        );
        assert_eq!(plain.note, Some(AddNote::TooSteep));
        // Forced, it hangs from that trunk.
        let (forced, _) = session.preview_add_with(tip, &attach).expect("a preview");
        assert!(
            forced.valid && forced.attached,
            "{:?}",
            (forced.valid, forced.attached, forced.note)
        );
        let mut edited = session.clone();
        edited.add_at(tip, &attach).expect("hung from the trunk");
        assert_eq!(new_knots(&session, &edited).len(), 1);
        assert_eq!(edited.state.collections.roots.len(), 1);
        assert!(edited.hand_edited());
        // Nothing in reach: red, with why, and a click places nothing.
        let far = [45.0, 5.0, 12.0];
        let (refused, kept) = session.preview_add_with(far, &attach).expect("a preview");
        assert!(!refused.valid && kept.is_none());
        assert_eq!(refused.note, Some(AddNote::TooFar));
        let mut untouched = session.clone();
        assert_eq!(
            untouched.add_at(far, &attach),
            Err(EditError::CannotAttach(AddNote::TooFar))
        );
        assert_eq!(untouched.state, session.state, "nothing placed");
    }

    #[test]
    fn a_hand_placed_support_takes_the_size_asked_for() {
        let tip_of = |session: &DfSupportSession| {
            let trunk = session.state.collections.trunks.values().next().unwrap();
            trunk
                .contact_cone
                .as_ref()
                .unwrap()
                .profile
                .contact_diameter_mm
        };
        let sized = |size: &str| {
            let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
            let how = AddOptions {
                attach: false,
                size: Some(size.to_owned()),
            };
            session.add_at([5.0, 5.0, 12.0], &how).expect("a trunk");
            tip_of(&session)
        };
        // Damian's four sizes (#360), on a preset that has none of its own.
        for (size, tip) in [
            ("extrasmall", 0.18),
            ("small", 0.24),
            ("medium", 0.30),
            ("large", 0.40),
        ] {
            assert!((sized(size) - tip).abs() < 1e-9, "{size}: {}", sized(size));
        }
        assert_eq!(
            bare_slab(12.0, crate::DfSupportRequest::default()).preset_hand_size(),
            None
        );
        // A zone preset gives its overhang size when none is asked for.
        let mut request = crate::DfSupportRequest::default();
        request.options.zones = Some(crate::zone_presets::minis_30());
        let mut zoned = bare_slab(12.0, request);
        assert_eq!(zoned.preset_hand_size(), Some("small"));
        zoned.add_trunk_at([5.0, 5.0, 12.0]).expect("a trunk");
        assert!((tip_of(&zoned) - 0.24).abs() < 1e-9, "{}", tip_of(&zoned));
    }

    /// What `weld_parts` did before #3, one piece after another: the
    /// parallel weld must give the very same mesh.
    fn weld_in_turn(soup: &[f32], parts: &[usize]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut vertices: Vec<[f32; 3]> = Vec::new();
        let mut faces = Vec::new();
        let mut seen: HashSet<Vec<u32>> = HashSet::new();
        let mut start = 0;
        for &end in parts.iter().chain(std::iter::once(&soup.len())) {
            let piece = &soup[start..end.max(start)];
            start = end.max(start);
            if piece.is_empty() || !seen.insert(piece.iter().map(|v| v.to_bits()).collect()) {
                continue;
            }
            let first = vertices.len();
            for tri in piece.chunks_exact(9) {
                let mut ids = [0u32; 3];
                for (k, p) in tri.chunks_exact(3).enumerate() {
                    let z = if p[2] <= 0.0 && p[2] > -crate::PLATE_SNAP_MM {
                        0.0
                    } else {
                        p[2]
                    };
                    let p = [p[0], p[1], z];
                    let tol = crate::WELD_TOLERANCE_MM;
                    let cell = p.map(|v| (v / tol).floor() as i64);
                    // The earliest vertex of the piece within reach, in the
                    // order the grid's neighbour cells were looked through.
                    let near = |q: &[f32; 3]| {
                        (0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>() <= tol * tol
                    };
                    let mut found = None;
                    'cells: for dx in -1..=1 {
                        for dy in -1..=1 {
                            for dz in -1..=1 {
                                let key = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                                for (id, q) in vertices.iter().enumerate().skip(first) {
                                    if q.map(|v| (v / tol).floor() as i64) == key && near(q) {
                                        found = Some(id as u32);
                                        break 'cells;
                                    }
                                }
                            }
                        }
                    }
                    ids[k] = found.unwrap_or_else(|| {
                        vertices.push(p);
                        (vertices.len() - 1) as u32
                    });
                }
                if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
                    faces.push(ids);
                }
            }
        }
        (vertices, faces)
    }

    #[test]
    fn the_parallel_weld_is_the_weld_in_turn() {
        let check = |soup: &[f32], parts: &[usize]| {
            let mesh = weld_parts(soup, parts, "t").expect("a mesh");
            let (vertices, faces) = weld_in_turn(soup, parts);
            let got: Vec<[f32; 3]> = mesh.vertices().iter().map(|v| *v.as_array()).collect();
            assert_eq!(got, vertices);
            let got: Vec<[u32; 3]> = mesh
                .faces()
                .iter()
                .map(|f| f.vertices.map(u32::from))
                .collect();
            assert_eq!(got, faces);
        };
        // A run's supports and raft.
        let mut request = crate::DfSupportRequest::default();
        request.options.zones = Some(crate::zone_presets::minis_30());
        let session = crate::start_df_supports(&slab_scene(6.0), request)
            .unwrap()
            .join()
            .unwrap()
            .sets
            .remove(0)
            .session;
        let sink = support_soup(&session.state, &session.models, &session.settings).unwrap();
        assert!(sink.parts.len() > 10);
        check(&sink.floats, &sink.parts);
        // Repeats, points either side of a cell, collapsed and sunk ones.
        let tri = |o: f32| [o, 0.0, 0.0, o + 1.0, 0.0, 0.0, o, 1.0, -5e-5];
        let tol = crate::WELD_TOLERANCE_MM;
        let mut soup: Vec<f32> = Vec::new();
        soup.extend(tri(0.0));
        soup.extend([0.0, 0.0, 0.0, 0.4 * tol, 0.0, 0.0, 1.0, 1.0, 0.0]);
        let first = soup.len();
        soup.extend(tri(0.0));
        soup.extend([0.0, 0.0, 0.0, 0.4 * tol, 0.0, 0.0, 1.0, 1.0, 0.0]);
        let second = soup.len();
        soup.extend(tri(3.0));
        soup.extend([3.0 + 0.6 * tol, 0.0, 0.0, 9.0, 9.0, 9.0, 3.0, 1.0, -5e-5]);
        check(&soup, &[first, second]);
    }

    /// A preview's pieces are drawn as before #3 drew them: a filtered copy
    /// of every collection, the models with their geometry.
    #[test]
    fn a_preview_draws_its_pieces_as_the_whole_models_do() {
        let session =
            crate::start_df_supports(&slab_scene(6.0), crate::DfSupportRequest::default())
                .unwrap()
                .join()
                .unwrap()
                .sets
                .remove(0)
                .session;
        let c = &session.state.collections;
        let some: HashSet<String> = all_ids(c).into_iter().step_by(3).collect();
        assert!(some.len() > 5);
        let keep = |id: &String| some.contains(id);
        assert_eq!(picked(c, keep), filtered(c, keep));
        let state = SupportState {
            collections: filtered(c, keep),
            ..SupportState::default()
        };
        let mut settings = (*session.settings).clone();
        settings.rafts.global.bottom_mode = RaftBottomMode::Off;
        settings.rafts.per_model.clear();
        let sink = support_soup(&state, &session.models, &settings).unwrap();
        let before = weld_parts(&sink.floats, &sink.parts, DF_SUPPORT_MESH_NAME).unwrap();
        let now = session.mesh_of(keep).unwrap();
        assert_eq!(now.vertices(), before.vertices());
        assert_eq!(now.faces(), before.faces());
    }

    #[test]
    fn fan_refusals_and_rejections_read_as_notes() {
        assert_eq!(AddNote::of_fan(FanLeafRefusal::NoHost), AddNote::TooFar);
        assert_eq!(AddNote::of_fan(FanLeafRefusal::Blocked), AddNote::HitsModel);
        assert_eq!(AddNote::of_fan(FanLeafRefusal::Build), AddNote::HitsModel);
        assert_eq!(
            AddNote::of_rejection(RejectReason::Trunk(LimitationCode::CollisionWithModel)),
            Some(AddNote::HitsModel)
        );
        assert_eq!(
            AddNote::of_rejection(RejectReason::SideWallContact),
            Some(AddNote::SteepSurface)
        );
        assert_eq!(AddNote::of_rejection(RejectReason::Exception), None);
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

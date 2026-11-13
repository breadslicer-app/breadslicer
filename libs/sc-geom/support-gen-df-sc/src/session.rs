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
use support_gen_df::features::slicing::support_world_triangles::raft_and_feet_top_mm;
use support_gen_df::pipeline::{ModelInput, PipelineSettings, support_soup};
use support_gen_df::supports::auto_bracing::auto_brace::{
    HandBrace, HandBraceRefusal, brace_new_shaft, hand_brace,
};
use support_gen_df::supports::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use support_gen_df::supports::auto_support::auto_place::fan::{
    FanArgs, FanHostOrder, FanLeafRefusal, FanLeafResult, add_fanned, fan_leaf_to_host,
};
use support_gen_df::supports::auto_support::auto_place::helpers::{
    FanShaftPoint, PlanCtx, PlanMesh, apply_zone_size, collect_fan_shaft_points, host_segment_span,
    member_max_angle_from_vertical_deg,
};
use support_gen_df::supports::auto_support::auto_place::place::{
    PlaceArgs, RejectReason, place_one_candidate,
};
use support_gen_df::supports::auto_support::parameter_sizing::SizingBands;
use support_gen_df::supports::auto_support::types::{CandidatePoint, CandidateSource};
use support_gen_df::supports::auto_support::zone_rules::{SupportSize, ZoneRules};
use support_gen_df::supports::placement_logic::cone_axis_policy::is_side_wall_contact;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::rafts::crenelated::raft_types::RaftBottomMode;
use support_gen_df::supports::support_type_registry::{
    SupportOriginId, SupportPlacementMetric, SupportPlacementSettingPath, SupportTypeId,
    near_plate_origin, select_type_for_placement,
};
use support_gen_df::supports::support_types::contact_bridge::{
    BridgeEntity, ContactBridgeRequest, build_contact_bridge,
};
use support_gen_df::supports::support_types::trunk::trunk_builder::{
    TrunkBuildInput, build_trunk_data,
};
use support_gen_df::supports::types::LimitationCode;
use support_gen_df::supports::types::{Knot, SupportCollections, SupportState};
use support_gen_df::three::ray::Ray;
use support_gen_df::three::{Matrix4, Vec3};
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};

use crate::document::ModelPrint;
use crate::hand_log::{
    HandBraced, HandEditKind, HandLog, HandPlaced, HandRemoved, HandUnbraced, KeptEdits, LostEdit,
    LostWhy,
};
use crate::shift::{PlaneMotion, bases, model_collections, move_model_supports};
use crate::{DF_SUPPORT_MESH_NAME, SettingsRecipe, weld_parts};

/// How far from the model surface a click may land and still place a support.
const ADD_REACH_MM: f64 = 2.0;
/// How far around a point the surface a tip sits on is read (#491): the
/// faces of a detailed sculpt are a few hundredths of a millimetre, smaller
/// than a pixel, and the one under the cursor may look up inside a surface
/// that looks down at the size of a tip and its cone. 0.4 mm is the
/// narrowest gap a support goes into: a surface closer than that across a
/// gap holds no support either way.
pub const COVERED_SURFACE_MM: f64 = 0.4;
/// How much the faces within [`COVERED_SURFACE_MM`] have to agree, the length
/// of their mean normal over their weight, for that mean to stand for the
/// surface: below it (a strand thinner than the ball, a sharp ridge) the
/// face itself does.
pub const COVERED_AGREEMENT: f64 = 0.25;
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
/// How far off a brace's surface a point may lie and still be on it, for
/// Remove (#3): the hit point of a ray, a little off the drawn facets.
const ON_BRACE_MM: f64 = 0.1;
/// How far from a support's axis a point of the brace tool may land and
/// still take that support (#3): the removal's reach.
const BRACE_REACH_MM: f64 = REMOVE_REACH_MM;
/// How far "Add support here", Fix All and the tips of an island may move a
/// check's point to one the engine can hold
/// ([`DfSupportSession::add_trunk_near`]). An island is often a fraction of a
/// millimetre across: the tip has to stay on it.
pub const REVIEW_NUDGE_MM: f64 = 0.3;

/// How much lower [`DfSupportSession::add_trunk_near`] may move a point the
/// engine cannot hold: a point a layer or two up the island's underside.
const NUDGE_DOWN_MM: [f64; 4] = [0.05, 0.1, 0.2, 0.4];
/// The directions [`DfSupportSession::add_trunk_near`] moves a point sideways.
const NUDGE_DIRECTIONS: usize = 8;
/// How far a nudged tip's face may turn from the aimed tip's and still be
/// the same underside: 20°, the cosine.
const NUDGE_SAME_FACE_COS: f64 = 0.94;
/// How far below the aimed tip a nudged one may land and still be on the
/// same underside: a layer.
const NUDGE_SAME_HEIGHT_MM: f64 = 0.05;
/// How far above the aimed tip a nudged one may land on the same face
/// (#405): two layers of 0.05 mm, where a growing island is larger than a
/// tip. The review's island policy aims a support no higher than that.
const NUDGE_CLIMB_MM: f64 = 0.10;
/// The heights above the point given that [`DfSupportSession::add_trunk_near`]
/// climbs to, once nothing at or below its height holds (#405).
const NUDGE_UP_MM: [f64; 2] = [0.05, 0.10];
/// How close two contacts are to count as the same tip: a nudge onto a flat
/// underside lands on the tip it was nudged from.
const SAME_TIP_MM: f64 = 1e-6;

#[path = "pieces.rs"]
mod pieces;

pub use pieces::{SupportContact, SupportJoint, SupportPiece, SupportPieceKind};

/// `settings`, with every tip ending in a ball on its contact point, so it goes
/// into the model by its radius, whatever made it (a run, a click, a tip moved,
/// Fix All): what the drawing and [`DfSupportSession::pieces`] read, so the
/// review sees the tip the printer cures. A session always runs on this, so it
/// holds for one reopened from a file too.
fn with_round_tips(mut settings: PipelineSettings) -> PipelineSettings {
    settings.tip_drawing.round_tip = true;
    settings
}

/// The contact diameter of the tips `settings` place.
pub(crate) fn tip_contact_diameter(settings: &PipelineSettings) -> f64 {
    let supports = &settings.supports;
    SizingBands::factory()
        .with_zone_sizes(supports.auto_support.zone_rules.as_ref())
        .active(supports)
        .tip_contact_diameter_mm
}

/// A model's BVH, built in the model's frame and shared by its copies
/// (#561), with the matrix placing this one: queries in world mm.
#[derive(Clone)]
pub(crate) struct PlacedBvh {
    pub(crate) bvh: Arc<MeshBvh>,
    pub(crate) to_world: Matrix4,
    to_local: Matrix4,
}

impl PlacedBvh {
    /// [`MeshBvh::closest_point_to_point`] of a world point, its hit in
    /// world mm. The placing matrix is a turn and a move: distances hold.
    pub(crate) fn closest_point_to_point(
        &self,
        point: Vec3,
        min_threshold: f64,
        max_threshold: f64,
    ) -> Option<support_gen_df::three_mesh_bvh::ClosestPointTarget> {
        let mut hit = self.bvh.closest_point_to_point(
            point.apply_matrix4(&self.to_local),
            min_threshold,
            max_threshold,
        )?;
        hit.point = hit.point.apply_matrix4(&self.to_world);
        Some(hit)
    }

    /// Face `face`, in world mm.
    pub(crate) fn triangle(&self, face: usize) -> support_gen_df::three::Triangle {
        let t = self.bvh.triangle(face);
        support_gen_df::three::Triangle::new(
            t.a.apply_matrix4(&self.to_world),
            t.b.apply_matrix4(&self.to_world),
            t.c.apply_matrix4(&self.to_world),
        )
    }

    /// [`MeshBvh::faces_within`] of a world point.
    pub(crate) fn faces_within(&self, point: Vec3, radius: f64, visit: impl FnMut(usize, f64)) {
        self.bvh
            .faces_within(point.apply_matrix4(&self.to_local), radius, visit);
    }

    /// [`MeshBvh::raycast_first`] of a world ray, its hit in world mm.
    pub(crate) fn raycast_first(
        &self,
        ray: &Ray,
        side: Side,
        near: f64,
        far: f64,
    ) -> Option<support_gen_df::three_mesh_bvh::Intersection> {
        let mut hit =
            self.bvh
                .raycast_first(&ray.apply_matrix4(&self.to_local), side, near, far)?;
        hit.point = hit.point.apply_matrix4(&self.to_world);
        hit.face.normal = hit.face.normal.transform_direction(&self.to_world);
        Some(hit)
    }
}

/// The BVHs copies share, while one of them holds each (#561): by the
/// model geometry they were built from.
static SHARED_BVHS: std::sync::Mutex<
    Vec<(
        std::sync::Weak<support_gen_df::three::buffer_geometry::BufferGeometry>,
        std::sync::Weak<MeshBvh>,
    )>,
> = std::sync::Mutex::new(Vec::new());

/// The BVH of `model`'s geometry in its own frame (re-centred on its box,
/// as the pipeline re-centres it): the one copies of the model built, or a
/// new one.
fn shared_bvh(model: &ModelInput) -> Arc<MeshBvh> {
    let held = |list: &Vec<(
        std::sync::Weak<support_gen_df::three::buffer_geometry::BufferGeometry>,
        std::sync::Weak<MeshBvh>,
    )>| {
        list.iter().find_map(|(g, b)| {
            g.upgrade()
                .is_some_and(|g| Arc::ptr_eq(&g, &model.geometry))
                .then(|| b.upgrade())
                .flatten()
        })
    };
    if let Some(bvh) = SHARED_BVHS.lock().ok().and_then(|list| held(&list)) {
        return bvh;
    }
    // Built with no lock held: another thread's query does not wait on it.
    let soup = support_gen_df::pipeline::local_soup(model);
    let triples: Vec<[f32; 3]> = soup.as_chunks::<3>().0.to_vec();
    let bvh = Arc::new(MeshBvh::new(triples, None));
    let Ok(mut list) = SHARED_BVHS.lock() else {
        return bvh;
    };
    if let Some(built) = held(&list) {
        return built;
    }
    list.retain(|(g, b)| g.strong_count() > 0 && b.strong_count() > 0);
    list.push((Arc::downgrade(&model.geometry), Arc::downgrade(&bvh)));
    bvh
}

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
    /// The hand edits as places on the model, to put back on a run made
    /// again (#408).
    hand_log: HandLog,
}

/// The first hand-placed id is the one after this.
pub(crate) const HAND_IDS_FROM: u64 = 1_000_000;

/// How far the model surface may lie from where a hand-placed tip touched
/// it, for a set made again to put the tip back there (#408).
const KEEP_SURFACE_MM: f64 = 0.05;
/// How far the surface's normal there may have turned.
const KEEP_NORMAL_DEG: f64 = 15.0;
/// How far off the surface, along the normal it had, a set made again aims
/// at a hand-placed tip: a tip on an edge then lands on the face it touched,
/// not on the wall beside it.
const KEEP_AIM_MM: f64 = 0.02;
/// How far from where a support of the old run had its tip a support of
/// the new run may have its own and be taken for it: the one a removal or
/// a move by hand acted on (#408).
const MATCH_TIP_MM: f64 = 1.0;

/// Where a set made again aims to put back a tip that touched the model at
/// `tip`, facing `normal`: [`KEEP_AIM_MM`] off the surface along it.
fn aim_at(tip: [f64; 3], normal: [f64; 3]) -> [f64; 3] {
    let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    if len <= 1e-12 {
        return tip;
    }
    std::array::from_fn(|k| tip[k] + normal[k] / len * KEEP_AIM_MM)
}

/// Whether `id` names an entity placed by hand: the `-m` ids
/// [`DfSupportSession::id_source`] gives.
fn is_hand_id(id: &str) -> bool {
    id.rsplit_once("-m")
        .and_then(|(_, n)| n.parse::<u64>().ok())
        .is_some_and(|n| n > HAND_IDS_FROM)
}

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
    /// A brace laid by hand cannot go there, for this reason (#3).
    CannotBrace(HandBraceRefusal),
    /// The engine could place a tip, but none of those tried holds what it
    /// was placed for (#433).
    NotHeld,
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
            EditError::CannotBrace(why) => write!(f, "no brace there ({why:?})"),
            EditError::NotHeld => write!(f, "no tip that fits reaches under what it must hold"),
        }
    }
}

/// The numbers a support placed by hand is drawn with, when they are the
/// person's own (the hand sizes of the Supports tab) rather than a size of
/// the run's preset.
#[derive(Debug, Clone, PartialEq)]
pub struct HandShape(pub SupportSize);

impl Eq for HandShape {}

impl std::hash::Hash for HandShape {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let s = &self.0;
        s.id.hash(state);
        for v in [
            s.tip_diameter_mm,
            s.tip_length_mm,
            s.penetration_mm,
            s.break_point_mm,
            s.trunk_diameter_mm,
            s.base_diameter_mm,
            s.base_thickness_mm,
            s.join_cone_mm,
            s.base_tip.map_or(-1.0, |t| t.length_mm),
            s.base_tip.map_or(-1.0, |t| t.diameter_mm),
        ] {
            v.to_bits().hash(state);
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
    /// The numbers it is drawn with, whatever the run's sizes are; goes
    /// before `size`.
    pub shape: Option<HandShape>,
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
    /// Other supports hang from the support being moved.
    CarriesOthers,
    /// The surface there faces up: a support from below cannot reach it.
    FacesUp,
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
    /// For a move: the pieces it takes away from where the support was.
    pub taken: Option<IndexedMesh>,
    /// For a brace laid by hand: why it cannot go there.
    pub brace: Option<HandBraceRefusal>,
}

/// One end of a brace laid by hand (#3): the support's shaft, and the point
/// on its axis nearest where the person pointed (world mm).
#[derive(Debug, Clone, PartialEq)]
pub struct BraceEnd {
    pub shaft: String,
    pub at: [f64; 3],
}

/// Where a support is placed: at the model surface nearest a point, or on
/// a contact known already (the model, the point, the outward normal).
#[derive(Clone, Copy)]
enum At {
    Point([f64; 3]),
    Contact(usize, Vec3, Vec3),
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
    if let Some(HandShape(shape)) = &how.shape {
        // The person's own numbers: a size of the rules that has them, else
        // one more size, under an id none of the run's has.
        let rules = auto.zone_rules.get_or_insert_with(ZoneRules::default);
        let same = |s: &SupportSize| {
            let mut s = s.clone();
            s.id.clone_from(&shape.id);
            s == *shape
        };
        if let Some(i) = rules.sizes.iter().position(same) {
            return Some(i);
        }
        let mut size = shape.clone();
        while rules.size_index(&size.id).is_some() {
            size.id.push_str("-hand");
        }
        rules.sizes.push(size);
        return Some(rules.sizes.len() - 1);
    }
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

/// Whether a nudged tip at `tip`, on a face of normal `normal`, holds what
/// the tip aimed at (`aimed`, on a face of normal `aimed_normal`) would:
/// the same underside, within `reach_mm` (#163). A face that looks down
/// within reach is not enough: a wall a step below the island, or a slope
/// beside it, takes a tip that holds nothing of the island. The tip has to
/// stay on a face turned as the aimed one is, at its height or up to
/// [`NUDGE_CLIMB_MM`] higher on it (#405).
fn same_underside(aimed: Vec3, aimed_normal: Vec3, tip: Vec3, normal: Vec3, reach_mm: f64) -> bool {
    let turn = normal.x * aimed_normal.x + normal.y * aimed_normal.y + normal.z * aimed_normal.z;
    normal.z < 0.0
        && turn >= NUDGE_SAME_FACE_COS
        && tip.z - aimed.z >= -NUDGE_SAME_HEIGHT_MM
        && tip.z - aimed.z <= NUDGE_CLIMB_MM
        && dist([tip.x, tip.y, tip.z], [aimed.x, aimed.y, aimed.z]) <= reach_mm
}

/// The points above `point` to try once [`nudges`] found nothing (#405):
/// the same spot and the rings around it out to `reach_mm`, at each of
/// [`NUDGE_UP_MM`], lowest first.
fn climbs(point: [f64; 3], reach_mm: f64) -> Vec<[f64; 3]> {
    let [x, y, z] = point;
    let rings = (reach_mm / 0.1 + 1e-9).floor() as usize;
    let mut out = Vec::new();
    for dz in NUDGE_UP_MM {
        out.push([x, y, z + dz]);
        for ring in 1..=rings {
            let r = ring as f64 * 0.1;
            for k in 0..NUDGE_DIRECTIONS {
                let a = k as f64 * std::f64::consts::TAU / NUDGE_DIRECTIONS as f64;
                out.push([x + r * a.cos(), y + r * a.sin(), z + dz]);
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
    dist(p, segment_nearest(p, a, b))
}

/// The point of the segment `a`-`b` nearest `p`.
fn segment_nearest(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if len2 <= 1e-18 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / len2)
        .clamp(0.0, 1.0);
    [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t]
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
            HandLog::default(),
        )
    }

    /// A session from its parts, with the next hand-placed id counting on
    /// from `next_id`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn restored(
        models: Vec<ModelInput>,
        settings: PipelineSettings,
        recipe: Option<SettingsRecipe>,
        state: SupportState,
        id_prefix: String,
        next_id: u64,
        hand_edited: bool,
        hand_log: HandLog,
    ) -> Self {
        let bvhs = (0..models.len()).map(|_| OnceLock::new()).collect();
        let prints = (0..models.len()).map(|_| OnceLock::new()).collect();
        let sdfs = (0..models.len()).map(|_| SdfSlot::default()).collect();
        DfSupportSession {
            removal_index: Arc::default(),
            models: models.into(),
            settings: Arc::new(with_round_tips(settings)),
            recipe: recipe.map(Arc::new),
            state,
            bvhs,
            prints,
            sdfs,
            id_prefix,
            next_id,
            hand_edited,
            hand_log,
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

    /// The session on the raft of `options` (#519): its kind, thickness and
    /// wall. The supports stay as they are, where they are; only the raft
    /// under them, and the roots' feet on it, are drawn again. `None` when
    /// the raft is already that one.
    pub fn with_raft_of(&self, options: &crate::DfSupportOptions) -> Option<Self> {
        let mut settings = (*self.settings).clone();
        if !options.apply_raft(&mut settings) {
            return None;
        }
        let mut next = self.clone();
        next.settings = Arc::new(settings);
        next.recipe = self.recipe.as_deref().map(|recipe| {
            let mut recipe = recipe.clone();
            let o = options.clamped();
            recipe.options.raft = o.raft;
            recipe.options.raft_thickness_mm = o.raft_thickness_mm;
            recipe.options.raft_wall = o.raft_wall;
            Arc::new(recipe)
        });
        Some(next)
    }

    /// Whether supports were added or removed by hand since the run: a run
    /// made again would not keep those edits.
    pub fn hand_edited(&self) -> bool {
        self.hand_edited
    }

    /// The hand edits as places on the model (#408).
    pub(crate) fn hand_log(&self) -> &HandLog {
        &self.hand_log
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

    /// The session with model `index` replaced by `now`, the same model
    /// with its mesh mended by Repair (#477): every support stays where it
    /// is. `None` when a support no longer touches the models: a point where
    /// one meets a model further than `tolerance_mm` from the models as they
    /// now are. `now` is the model as a run makes it from the scene; it
    /// keeps the session's id. The BVH of the mended model is built here, so
    /// the next edit finds it ready.
    pub fn rebased(&self, index: usize, mut now: ModelInput, tolerance_mm: f64) -> Option<Self> {
        now.id = self.models.get(index)?.id.clone();
        let rebased = self.taken_along(index, now, &PlaneMotion::shift([0.0, 0.0]));
        let touches = |p: Vec3| {
            (0..rebased.models.len()).any(|i| {
                rebased
                    .bvh(i)
                    .closest_point_to_point(p, 0.0, tolerance_mm)
                    .is_some_and(|hit| hit.distance <= tolerance_mm)
            })
        };
        rebased
            .model_contacts()
            .into_iter()
            .all(touches)
            .then_some(rebased)
    }

    /// Every point where a support meets a model (world mm): the tips of
    /// trunks, branches, leaves and stumps, and both ends of the twigs and
    /// sticks that join model to model.
    fn model_contacts(&self) -> Vec<Vec3> {
        let c = &self.state.collections;
        let trunks = c.trunks.values().filter_map(|t| t.contact_cone.as_ref());
        let branches = c.branches.values().filter_map(|b| b.contact_cone.as_ref());
        let leaves = c.leaves.values().map(|l| &l.contact_cone);
        let stumps = c.stumps.values().map(|s| &s.contact_cone);
        let sticks = c
            .sticks
            .values()
            .flat_map(|s| [&s.contact_cone_a, &s.contact_cone_b]);
        trunks
            .chain(branches)
            .chain(leaves)
            .chain(stumps)
            .chain(sticks)
            .map(|cone| cone.pos)
            .chain(
                c.twigs
                    .values()
                    .flat_map(|t| [t.contact_disk_a.pos, t.contact_disk_b.pos]),
            )
            .collect()
    }

    /// The session with model `index` replaced by `model`, its supports
    /// taken along by `motion`.
    fn taken_along(&self, index: usize, model: ModelInput, motion: &PlaneMotion) -> Self {
        let mut state = self.state.clone();
        move_model_supports(&mut state.collections, &model.id, motion);
        let hand_log = self.hand_log.moved(&model.id, motion);
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
            hand_log,
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
                let hand_log = self.hand_log.of_model(&model.id);
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
                    hand_log,
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

    /// What the session holds on the heap that may be shared with other
    /// sessions, as (part, address, bytes): each model's geometry and each
    /// BVH built. Parts at the same address are one in memory (#561).
    pub fn memory_parts(&self) -> Vec<(&'static str, usize, usize)> {
        let mut parts = Vec::new();
        for m in self.models.iter() {
            let g = &m.geometry;
            let bytes = g.position.as_ref().map_or(0, |p| p.capacity() * 4)
                + g.index.as_ref().map_or(0, |i| i.capacity() * 4)
                + g.normal.as_ref().map_or(0, |n| n.capacity() * 4);
            parts.push(("model", Arc::as_ptr(g) as usize, bytes));
        }
        for slot in self.bvhs.iter() {
            if let Some(bvh) = slot.get() {
                parts.push(("bvh", Arc::as_ptr(bvh) as usize, bvh.heap_bytes()));
            }
        }
        parts.push((
            "sdf",
            Arc::as_ptr(&self.sdfs) as *const u8 as usize,
            self.sdfs
                .iter()
                .filter(|s| s.0.lock().is_ok_and(|s| s.is_some()))
                .count(),
        ));
        parts
    }

    /// Model `i`'s BVH, in world mm: built in the model's own frame, once
    /// for every copy that shares its geometry (#561), and placed where this
    /// session's model stands.
    fn bvh(&self, i: usize) -> PlacedBvh {
        let model = &self.models[i];
        let bvh = self.bvhs[i].get_or_init(|| shared_bvh(model)).clone();
        let to_world = support_gen_df::pipeline::model_matrix(&model.transform);
        PlacedBvh {
            to_local: to_world.invert(),
            to_world,
            bvh,
        }
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

    /// Why a tip may not touch a surface of normal `n`: it looks up, or it
    /// is a wall, which a run keeps its trunks off and a tip placed by hand
    /// keeps off too.
    fn untouchable(&self, n: Vec3) -> Option<AddNote> {
        let tip = &self.settings.supports.tip;
        if n.z > 0.0 {
            Some(AddNote::FacesUp)
        } else if is_side_wall_contact(
            n,
            tip.cone_angle_mode.unwrap_or_default(),
            tip.adaptive_cone_angle_offset_deg,
        ) {
            Some(AddNote::SteepSurface)
        } else {
            None
        }
    }

    /// Where a tip aimed at `point` touches the model: [`Self::contact`],
    /// and the normal it takes there, or why it may not touch. The face's,
    /// when a tip may touch it; else the surface the tip covers
    /// ([`Self::covered_normal`]), when a tip may touch that (#491): on a
    /// detailed sculpt the face under the cursor is a facet smaller than a
    /// pixel that may look up, or sideways, inside a surface that looks
    /// down. A refusal tells of the surface.
    fn tip_contact(&self, point: [f64; 3]) -> Option<(usize, Vec3, Result<Vec3, AddNote>)> {
        let (i, at, face) = self.contact(point)?;
        if self.untouchable(face).is_none() {
            return Some((i, at, Ok(face)));
        }
        let covered = self.covered_normal(i, at, face);
        Some((i, at, self.untouchable(covered).map_or(Ok(covered), Err)))
    }

    /// The normal of the surface around `at` on model `i`, a point on a face
    /// of normal `face` (#491): the mean of the faces within
    /// [`COVERED_SURFACE_MM`], each weighted by its area inside that ball. A
    /// face counts at most the disc the ball cuts from its plane, so the big
    /// faces of a plain model weigh what a fine mesh's would. `face` itself
    /// when they disagree too much to stand for one surface.
    fn covered_normal(&self, i: usize, at: Vec3, face: Vec3) -> Vec3 {
        let bvh = self.bvh(i);
        let reach_sq = COVERED_SURFACE_MM * COVERED_SURFACE_MM;
        let mut sum = Vec3::ZERO;
        let mut weight = 0.0;
        bvh.faces_within(at, COVERED_SURFACE_MM, |f, d| {
            let tri = bvh.triangle(f);
            let n = Vec3::sub_vectors(tri.b, tri.a).cross(Vec3::sub_vectors(tri.c, tri.a));
            let area = n.length() / 2.0;
            if area <= 1e-18 {
                return;
            }
            let w = area.min(std::f64::consts::PI * (reach_sq - d * d).max(0.0));
            sum = sum.add_scaled_vector(n, w / (2.0 * area));
            weight += w;
        });
        if weight > 0.0 && sum.length() >= COVERED_AGREEMENT * weight {
            sum.normalize()
        } else {
            face
        }
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
        self.place_at(At::Point(point), how)
    }

    /// [`Self::place`], at a point to find the surface under, or on a
    /// contact known already: a hand edit put back on a run made again
    /// (#408) touches the model where and how it did, as a tip on an edge
    /// between two faces would land on either.
    fn place_at(
        &mut self,
        at: At,
        how: &AddOptions,
    ) -> Result<Placed, (EditError, Option<AddNote>)> {
        // The tip goes exactly where the person points (#3): a surface that
        // looks up is no place for one, nor is a wall (a run keeps its
        // trunks off one), and that is said rather than the tip going to
        // some other surface near it.
        let refused = |note: AddNote| {
            let why = match note {
                AddNote::FacesUp => "the surface faces up".to_owned(),
                _ => RejectReason::SideWallContact.as_str().to_owned(),
            };
            Err((EditError::NoRoute(why), Some(note)))
        };
        let (i, tip, normal) = match at {
            At::Point(point) => match self.tip_contact(point) {
                Some((i, tip, Ok(normal))) => (i, tip, normal),
                Some((_, _, Err(note))) => return refused(note),
                None => return Err((EditError::NothingThere, None)),
            },
            At::Contact(i, tip, normal) => match self.untouchable(normal) {
                Some(note) => return refused(note),
                None => (i, tip, normal),
            },
        };
        let bvh = self.bvh(i);
        let mut settings = (*self.settings).clone();
        let zone = hand_settings(&mut settings, how);
        let model_id = self.models[i].id.clone();
        let before = all_ids(&self.state.collections);
        let pooled = self.sdfs[i].0.lock().ok().and_then(|mut slot| slot.take());
        let sdf = RefCell::new(pooled.unwrap_or_else(|| {
            SdfCache::new(bvh.bvh.clone(), bvh.to_world, Some(settings.sdf_cell_mm))
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
                    bvh: &bvh.bvh,
                    matrix: bvh.to_world,
                    side: Side::Front,
                    normals: None,
                    sdf: &sdf,
                    blocked: None,
                }),
                &mut new_id,
            );
            ctx.set_zone(zone);
            ctx.set_exact_contact(Some((tip, normal)));
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
                    bvh: &bvh.bvh,
                    matrix: bvh.to_world,
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
        // The log keeps the support as a place on the model (#408).
        if let Some(id) = added
            .iter()
            .filter(|id| self.support_tip(id).is_some())
            .min()
            .cloned()
        {
            self.hand_log.placed.push(HandPlaced {
                id,
                model: self.models[i].id.clone(),
                tip: [tip.x, tip.y, tip.z],
                normal: [normal.x, normal.y, normal.z],
                attach: how.attach,
                size: how.size.clone(),
                shape: how.shape.clone().map(|s| s.0),
                moved_from: None,
            });
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
    /// engine can hold on the same underside, whose tip lands within
    /// `reach_mm` of the one `point` gives: up to 0.4 mm lower and `reach_mm`
    /// to the side. For a
    /// point a check picked rather than a click (the Review card's islands):
    /// the route from one tip can graze the model where a tip a step away
    /// clears it. A tip on some other face near it (a wall below the island,
    /// a slope beside it) would hold nothing of what was asked for, and is
    /// not taken (#163). When nothing at or below that height holds, it
    /// climbs the same face up to 0.10 mm, two layers, where a growing
    /// island is larger and its route may clear the model (#405). Returns the
    /// point used; the error is the one at `point`.
    pub fn add_trunk_near(
        &mut self,
        point: [f64; 3],
        reach_mm: f64,
    ) -> Result<[f64; 3], EditError> {
        self.place_near(point, reach_mm, &AddOptions::default(), &mut |_, _| true)
    }

    /// [`Self::add_trunk_near`], taking only a support `keep` keeps. One it
    /// does not goes on trying, and `keep` is the one to put the session back.
    fn place_near(
        &mut self,
        point: [f64; 3],
        reach_mm: f64,
        how: &AddOptions,
        keep: &mut dyn FnMut(&mut Self, &Placed) -> bool,
    ) -> Result<[f64; 3], EditError> {
        let Some((_, aimed, normal)) = self.contact(point) else {
            return Err(EditError::NothingThere);
        };
        // A face that looks up holds nothing above it: only the points next
        // to it are tried.
        let mut first = EditError::NoRoute("the nearest face looks up".to_owned());
        // Contacts already tried: a nudge onto one of them places the very
        // same support, refused the same way.
        let mut tried: Vec<(Vec3, Vec3)> = Vec::new();
        let seen = |tried: &[(Vec3, Vec3)], tip: Vec3, n: Vec3| {
            tried.iter().any(|(t, m)| {
                dist([t.x, t.y, t.z], [tip.x, tip.y, tip.z]) <= SAME_TIP_MM
                    && dist([m.x, m.y, m.z], [n.x, n.y, n.z]) <= SAME_TIP_MM
            })
        };
        if normal.z < 0.0 {
            match self.place(point, how) {
                Ok(placed) if keep(self, &placed) => return Ok(point),
                Ok(_) => {}
                Err((err @ EditError::NoRoute(_), _)) => first = err,
                Err((other, _)) => return Err(other),
            }
            tried.push((aimed, normal));
        }
        for q in nudges(point, reach_mm)
            .into_iter()
            .chain(climbs(point, reach_mm))
        {
            let Some((_, tip, n)) = self.contact(q) else {
                continue;
            };
            if seen(&tried, tip, n) || !same_underside(aimed, normal, tip, n, reach_mm) {
                continue;
            }
            if let Ok(placed) = self.place(q, how)
                && keep(self, &placed)
            {
                return Ok(q);
            }
            tried.push((tip, n));
        }
        Err(first)
    }

    /// Adds a tip a check asked for at `point`, or on the first of `others`
    /// that works, tried as they are, or as [`Self::add_trunk_near`] does
    /// around `point` ([`REVIEW_NUDGE_MM`]): "Add support here", Fix All and
    /// the tips of an island (#437). `others` come first: they lie on the
    /// island's own pixels, and each step aside costs a placement, a hundred
    /// of them on a start the shaft cannot clear (#433). At the very bottom
    /// of a narrow start the shaft often grazes the island, and a step aside
    /// clears it.
    ///
    /// Works means the engine holds the point and `holds` says the contacts
    /// of what it added do what the tip is for: placing and checking are one
    /// step (#433). A tip `holds` turns down leaves the session as it was.
    /// The tip takes `size`, or the preset's. Last of all each point is tried
    /// hanging from the nearest support, as Shift+click does: a tip with no way
    /// of its own to the plate may still reach one. Returns the point used.
    /// The error is [`EditError::NotHeld`] when tips went in and none held,
    /// otherwise the one at `point`.
    pub fn add_tip_among(
        &mut self,
        point: [f64; 3],
        others: &[[f64; 3]],
        size: Option<&str>,
        holds: &dyn Fn(&[SupportContact]) -> bool,
    ) -> Result<[f64; 3], EditError> {
        let how = AddOptions {
            size: size.map(str::to_owned),
            ..AddOptions::default()
        };
        let before = self.clone();
        let mut turned_down = false;
        let mut keep = |session: &mut Self, placed: &Placed| {
            let held = holds(&session.contacts_of(&placed.added));
            if !held {
                *session = before.clone();
                turned_down = true;
            }
            held
        };
        for at in std::iter::once(&point).chain(others) {
            if let Ok(placed) = self.place(*at, &how)
                && keep(self, &placed)
            {
                return Ok(*at);
            }
        }
        let near = self.place_near(point, REVIEW_NUDGE_MM, &how, &mut keep);
        if near.is_ok() {
            return near;
        }
        let hanging = AddOptions {
            attach: true,
            ..how
        };
        for at in std::iter::once(&point).chain(others) {
            if let Ok(placed) = self.place(*at, &hanging)
                && keep(self, &placed)
            {
                return Ok(*at);
            }
        }
        match near {
            Err(_) if turned_down => Err(EditError::NotHeld),
            other => other,
        }
    }

    /// How far each tip goes past the surface it touches, and whether its end
    /// lies inside the model, measured on the mesh: what says a support
    /// ends in the model.
    pub fn tip_depths(&self) -> Vec<(f64, bool)> {
        self.pieces()
            .iter()
            .flat_map(|piece| &piece.contacts)
            .map(|contact| {
                let [x, y, z] = contact.tip;
                let end = Vec3::new(x, y, z);
                let inside = (0..self.models.len()).any(|i| {
                    let bvh = self.bvh(i);
                    bvh.closest_point_to_point(end, 0.0, 2.0)
                        .is_some_and(|hit| {
                            let tri = bvh.triangle(hit.face_index);
                            let n = Vec3::sub_vectors(tri.b, tri.a)
                                .cross(Vec3::sub_vectors(tri.c, tri.a));
                            n.dot(Vec3::sub_vectors(end, hit.point)) < 0.0
                        })
                });
                (contact.inserted_mm, inside)
            })
            .collect()
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
                    taken: None,
                    brace: None,
                };
                Ok((preview, Some(copy)))
            }
            Err((EditError::NoRoute(_) | EditError::CannotAttach(_), note)) => {
                let preview = EditPreview {
                    mesh: self.refused_at(point, note, how)?,
                    valid: false,
                    attached: false,
                    note,
                    taken: None,
                    brace: None,
                };
                Ok((preview, None))
            }
            Err((other, _)) => Err(other),
        }
    }

    /// What a preview shows red where nothing can hold `point`, refused as
    /// `note` says (#491). A surface no tip may touch (it looks up, or it is
    /// a wall) shows no support: none was tried. Otherwise the support
    /// straight down to what lies under the point ([`Self::straight_drop`]),
    /// of the size `how` asks for.
    fn refused_at(
        &self,
        point: [f64; 3],
        note: Option<AddNote>,
        how: &AddOptions,
    ) -> Result<IndexedMesh, EditError> {
        match note {
            Some(AddNote::FacesUp | AddNote::SteepSurface) => {
                self.contact(point).ok_or(EditError::NothingThere)?;
                Ok(IndexedMesh::new(None, Vec::new(), Vec::new()))
            }
            _ => self.straight_drop(point, how),
        }
    }

    /// The support straight down from the model surface at `point` to what
    /// lies under it, drawn on its own (#491): to the model right below, a
    /// support from one surface to the other, as the placement tries there;
    /// else a trunk to the plate. A red preview drawn as a trunk to the
    /// plate through the model below read as a support sent to the plate.
    fn straight_drop(&self, point: [f64; 3], how: &AddOptions) -> Result<IndexedMesh, EditError> {
        let (i, tip, face) = self.contact(point).ok_or(EditError::NothingThere)?;
        // Of the size asked for (#564: a red one showed the run's size, not
        // the one a click would place).
        let mut settings = (*self.settings).clone();
        if let Some(zone) = hand_settings(&mut settings, how)
            && let Some(rules) = settings.supports.auto_support.zone_rules.clone()
        {
            apply_zone_size(&mut settings.supports, &rules, zone);
        }
        let supports = &settings.supports;
        let sized = settings
            .supports
            .auto_support
            .zone_rules
            .is_some()
            .then_some((supports.shaft.diameter_mm, supports.tip.contact_diameter_mm));
        let normal = match self.tip_contact(point) {
            Some((_, _, Ok(n))) => n,
            _ => face,
        };
        let mut n = 0;
        let mut new_id = || {
            n += 1;
            format!("preview-{n}")
        };
        let bvh = self.bvh(i);
        // Past the tip's own surface: the bridge looks from a tenth of a
        // millimetre under the contact.
        let below = bvh.raycast_first(
            &Ray {
                origin: tip,
                direction: Vec3::new(0.0, 0.0, -1.0),
            },
            Side::Double,
            0.1,
            f64::INFINITY,
        );
        if let Some(hit) = below {
            let tri = bvh.triangle(hit.face_index);
            let up = Vec3::sub_vectors(tri.b, tri.a)
                .cross(Vec3::sub_vectors(tri.c, tri.a))
                .normalize();
            let span = tip.distance_to(hit.point);
            let cutoff = supports.mesh_to_mesh.stick_vs_twig_cutoff_mm;
            let read_setting = |path: SupportPlacementSettingPath| match path {
                SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm => Some(cutoff),
            };
            let kind =
                select_type_for_placement(SupportPlacementMetric::ContactSpan, span, &read_setting)
                    .unwrap_or(SupportTypeId::Stick);
            let bridge = build_contact_bridge(
                kind,
                &ContactBridgeRequest {
                    model_id: &self.models[i].id,
                    a_pos: tip,
                    a_normal: normal,
                    b_pos: hit.point,
                    b_normal: up,
                    shaft_diameter_mm: sized.map(|s| s.0),
                    tip_contact_diameter_mm: sized.map(|s| s.1),
                    mesh: None,
                    manual: Some(true),
                },
                supports,
                &mut new_id,
            );
            if let Some(bridge) = bridge {
                let mut state = SupportState::default();
                let c = &mut state.collections;
                match bridge.entity {
                    BridgeEntity::Twig(t) => {
                        c.twigs.insert(t.base.id.clone(), t);
                    }
                    BridgeEntity::Stick(s) => {
                        c.sticks.insert(s.base.id.clone(), s);
                    }
                }
                return self.draw_alone(&state);
            }
        }
        let built = build_trunk_data(
            &TrunkBuildInput {
                tip_pos: tip,
                tip_normal: normal,
                model_id: &self.models[i].id,
                mesh: None,
                is_preview: Some(true),
                overrides: None,
                is_small_island: None,
                ignore_grid: Some(true),
            },
            supports,
            &mut new_id,
        )
        .ok_or(EditError::NothingThere)?;
        let mut state = SupportState::default();
        let c = &mut state.collections;
        c.roots.insert(built.root.base.id.clone(), built.root);
        c.trunks.insert(built.trunk.base.id.clone(), built.trunk);
        self.draw_alone(&state)
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
            // A brace holds no point of its own: it runs between its knots
            // (#3). It is taken only for a point on its own surface: a radius
            // off the part of it out in the open, its ends inside the shafts
            // it joins left out. A point on a shaft beside where a brace
            // meets it is the shaft's.
            if let Some(brace) = c.braces.get(&index.ids[k]) {
                let Some(d) = self.on_brace(brace, point) else {
                    continue;
                };
                if d < best.map_or(f64::INFINITY, |b| b.0) {
                    best = Some((d, k));
                }
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
        let Some((d, start)) = self.nearest_support(point) else {
            return Err(EditError::NothingThere);
        };
        if d > REMOVE_REACH_MM {
            return Err(EditError::NothingThere);
        }
        Ok(self.cascade(&[start]))
    }

    /// The ids removing the entities at `starts` (places in the removal
    /// index) takes.
    fn cascade(&self, starts: &[usize]) -> HashSet<String> {
        let index = self.removal_index();
        let c = &self.state.collections;
        // The cascade: what names the removed entity or any of its parts (a
        // knot names the shaft segment it sits on) goes with it.
        let n = index.ids.len();
        let mut gone = vec![false; n];
        for &start in starts {
            gone[start] = true;
        }
        let mut queue = starts.to_vec();
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
        (0..n)
            .filter(|&k| gone[k])
            .map(|k| index.ids[k].clone())
            .collect()
    }

    /// Every support's tip on the model, with its id: the trunks', branches'
    /// and leaves' contact points (world mm). What a box or a brush removes
    /// supports by, and what Move tip grabs (#3).
    pub fn tips(&self) -> Vec<(String, [f64; 3])> {
        let c = &self.state.collections;
        let at = |p: Vec3| [p.x, p.y, p.z];
        c.trunks
            .iter()
            .filter_map(|(id, t)| t.contact_cone.as_ref().map(|k| (id.clone(), at(k.pos))))
            .chain(
                c.branches
                    .iter()
                    .filter_map(|(id, b)| b.contact_cone.as_ref().map(|k| (id.clone(), at(k.pos)))),
            )
            .chain(
                c.leaves
                    .iter()
                    .map(|(id, l)| (id.clone(), at(l.contact_cone.pos))),
            )
            .collect()
    }

    /// Every tip with the outward normal of the surface it touches, where
    /// it touches (#524): the places a run picked, for showing them.
    pub fn tip_contacts(&self) -> Vec<([f64; 3], [f64; 3])> {
        let c = &self.state.collections;
        let of =
            |k: &support_gen_df::supports::support_primitives::contact_cone::types::ContactCone| {
                // The cone's axis points into the model: the surface normal is
                // turned to face the other way, out of it.
                let out = [-k.normal.x, -k.normal.y, -k.normal.z];
                let n = k.surface_normal.map_or(out, |n| {
                    let n = [n.x, n.y, n.z];
                    if n[0] * out[0] + n[1] * out[1] + n[2] * out[2] < 0.0 {
                        n.map(|c| -c)
                    } else {
                        n
                    }
                });
                ([k.pos.x, k.pos.y, k.pos.z], n)
            };
        c.trunks
            .values()
            .filter_map(|t| t.contact_cone.as_ref().map(of))
            .chain(
                c.branches
                    .values()
                    .filter_map(|b| b.contact_cone.as_ref().map(of)),
            )
            .chain(c.leaves.values().map(|l| of(&l.contact_cone)))
            .collect()
    }

    /// Every support the person sees as one, with its id and the line it
    /// runs along (world mm), from its tip down: its joints, then the root it
    /// stands on or the knot it hangs from. What a box or a brush removes by
    /// (#3): a tip touching the model's underside is mostly hidden behind
    /// it, and the shaft below is what is on screen.
    pub fn support_paths(&self) -> Vec<(String, Vec<[f64; 3]>)> {
        let c = &self.state.collections;
        let at = |p: Vec3| [p.x, p.y, p.z];
        let joints = |segments: &[support_gen_df::supports::types::Segment]| {
            segments
                .iter()
                .flat_map(|s| [s.top_joint.as_ref(), s.bottom_joint.as_ref()])
                .flatten()
                .map(|j| at(j.pos))
                .collect::<Vec<_>>()
        };
        let knot = |id: &str| c.knots.get(id).map(|k| at(k.pos));
        let path = |tip: Option<Vec3>, mut points: Vec<[f64; 3]>, end: Option<[f64; 3]>| {
            points.extend(tip.map(at));
            points.extend(end);
            points.sort_by(|a, b| b[2].total_cmp(&a[2]));
            points.dedup();
            points
        };
        let mut out = Vec::new();
        for (id, t) in &c.trunks {
            let root = c.roots.get(&t.root_id).map(|r| at(r.transform.pos));
            let points = path(
                t.contact_cone.as_ref().map(|k| k.pos),
                joints(&t.segments),
                root,
            );
            out.push((id.clone(), points));
        }
        for (id, b) in &c.branches {
            let points = path(
                b.contact_cone.as_ref().map(|k| k.pos),
                joints(&b.segments),
                knot(&b.parent_knot_id),
            );
            out.push((id.clone(), points));
        }
        for (id, l) in &c.leaves {
            let points = path(
                Some(l.contact_cone.pos),
                Vec::new(),
                knot(&l.parent_knot_id),
            );
            out.push((id.clone(), points));
        }
        out
    }

    /// The support whose tip is nearest `point` (world mm), within
    /// `reach_mm`.
    pub fn tip_near(&self, point: [f64; 3], reach_mm: f64) -> Option<String> {
        self.tips()
            .into_iter()
            .map(|(id, tip)| (dist(point, tip), id))
            .filter(|(d, _)| *d <= reach_mm)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// Where support `id`'s tip touches the model, and the model it stands
    /// on (its id in the state): a trunk's, a branch's or a leaf's; `None`
    /// for anything else.
    fn support_tip(&self, id: &str) -> Option<([f64; 3], &str)> {
        let c = &self.state.collections;
        let at = |p: Vec3| [p.x, p.y, p.z];
        if let Some(t) = c.trunks.get(id) {
            return t
                .contact_cone
                .as_ref()
                .map(|k| (at(k.pos), t.base.model_id.as_str()));
        }
        if let Some(b) = c.branches.get(id) {
            return b
                .contact_cone
                .as_ref()
                .map(|k| (at(k.pos), b.base.model_id.as_str()));
        }
        c.leaves
            .get(id)
            .map(|l| (at(l.contact_cone.pos), l.base.model_id.as_str()))
    }

    /// Notes in the log a removal of the supports `picked` that takes the
    /// entities `gone` with it (#408): a support of the run picked is
    /// logged as removed, by where its tip was; what hangs from it goes with
    /// it again when the removal is made on a run made again. A support
    /// placed by hand that goes leaves the log; one of the run moved by hand
    /// leaves its old place removed.
    fn log_removal(&mut self, picked: &[String], gone: &HashSet<String>) {
        self.forget_braces(gone);
        for id in picked {
            if !gone.contains(id) || is_hand_id(id) {
                continue;
            }
            let c = &self.state.collections;
            if let (Some(brace), Some([from, to])) = (c.braces.get(id), self.brace_ends(id)) {
                let model = brace.base.model_id.clone();
                self.hand_log
                    .unbraced
                    .push(HandUnbraced { model, from, to });
            }
        }
        let mut ids: Vec<&String> = gone.iter().collect();
        ids.sort();
        for id in ids {
            if let Some(k) = self.hand_log.placed.iter().position(|p| p.id == *id) {
                let placed = self.hand_log.placed.remove(k);
                if let Some(tip) = placed.moved_from {
                    self.hand_log.removed.push(HandRemoved {
                        model: placed.model,
                        tip,
                    });
                }
            }
        }
        for id in picked {
            if !gone.contains(id) || is_hand_id(id) {
                continue;
            }
            if let Some((tip, model)) = self.support_tip(id) {
                let model = model.to_owned();
                self.hand_log.removed.push(HandRemoved { model, tip });
            }
        }
    }

    /// The support of the run (not one placed by hand) whose tip is
    /// nearest `point`, within [`MATCH_TIP_MM`].
    fn run_tip_near(&self, point: [f64; 3]) -> Option<String> {
        self.tips()
            .into_iter()
            .filter(|(id, _)| !is_hand_id(id))
            .map(|(id, tip)| (dist(point, tip), id))
            .filter(|(d, _)| *d <= MATCH_TIP_MM)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// The model whose surface still lies where a tip touched it, at
    /// `tip`, facing the way it did (`normal`), as a click at `aim` finds
    /// it; `None` when the surface there moved or turned.
    fn surface_holds(&self, aim: [f64; 3], tip: [f64; 3], normal: [f64; 3]) -> Option<usize> {
        let (model, at, n) = self.tip_contact(aim)?;
        let n = n.ok()?;
        let was = Vec3::new(normal[0], normal[1], normal[2]);
        let len = was.length();
        (len > 1e-12
            && dist([at.x, at.y, at.z], tip) <= KEEP_SURFACE_MM
            && n.dot(was) / len >= KEEP_NORMAL_DEG.to_radians().cos())
        .then_some(model)
    }

    /// The hand edits to put back on a run made again (#408). A set an
    /// older build saved recorded none: its supports placed by hand, known
    /// by their ids, are taken as added where their tips touch the model.
    fn log_to_keep(&self) -> HandLog {
        let mut log = self.hand_log.clone();
        if !log.unrecorded {
            return log;
        }
        let logged: HashSet<&str> = log.placed.iter().map(|p| p.id.as_str()).collect();
        let mut found = Vec::new();
        let mut tips = self.tips();
        tips.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, tip) in tips {
            if !is_hand_id(&id) || logged.contains(id.as_str()) {
                continue;
            }
            let (Some((_, at, Ok(n))), Some((_, model))) =
                (self.tip_contact(tip), self.support_tip(&id))
            else {
                continue;
            };
            found.push(HandPlaced {
                model: model.to_owned(),
                tip: [at.x, at.y, at.z],
                normal: [n.x, n.y, n.z],
                attach: false,
                size: self.size_of(&id),
                shape: None,
                moved_from: None,
                id,
            });
        }
        log.placed.splice(0..0, found);
        log
    }

    /// This session, a run made again on the model `old`'s supports stood
    /// on, with `old`'s hand edits (#3) put back where they still fit
    /// (#408): each support of the old run removed by hand takes the new
    /// run's support at that place with it, if there is one; each support
    /// added by hand is added again, by the same rules as a click, where the
    /// model surface still lies and faces as it did; each support of the
    /// old run moved by hand has the new run's support at that place moved
    /// the same way. The edits that no longer fit are reported, the others
    /// counted. `None` when `cancelled` says so, between two edits.
    pub fn with_hand_edits_of(
        &self,
        old: &DfSupportSession,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<(Self, KeptEdits)> {
        let mut session = self.clone();
        let log = old.log_to_keep();
        let mut report = KeptEdits {
            unrecorded: log.unrecorded,
            ..KeptEdits::default()
        };
        for removed in &log.removed {
            if cancelled() {
                return None;
            }
            // With no support of the new run there, nothing is to remove.
            if let Some(id) = session.run_tip_near(removed.tip) {
                let _ = session.remove_tips(&[id]);
            }
            report.kept += 1;
        }
        for placed in &log.placed {
            if cancelled() {
                return None;
            }
            let kind = if placed.moved_from.is_some() {
                HandEditKind::Moved
            } else {
                HandEditKind::Added
            };
            let lost = |why| LostEdit {
                kind,
                at: placed.tip,
                why,
            };
            let aim = aim_at(placed.tip, placed.normal);
            let Some(model) = session.surface_holds(aim, placed.tip, placed.normal) else {
                report.lost.push(lost(LostWhy::SurfaceChanged));
                continue;
            };
            let [x, y, z] = placed.tip;
            let [nx, ny, nz] = placed.normal;
            let contact = At::Contact(model, Vec3::new(x, y, z), Vec3::new(nx, ny, nz).normalize());
            let how = AddOptions {
                attach: placed.attach,
                size: placed.size.clone(),
                shape: placed.shape.clone().map(HandShape),
            };
            // A move takes the new run's support where the old one stood;
            // with none there, the support is added where it was moved to.
            let done = match placed
                .moved_from
                .and_then(|from| session.run_tip_near(from))
            {
                Some(id) => {
                    let mut moved = session.clone();
                    moved
                        .move_in_place(&id, contact, &how)
                        .map(|_| session = moved)
                }
                None => {
                    let mut added = session.clone();
                    added.place_at(contact, &how).map(|_| session = added)
                }
            }
            .map_err(|(err, _)| err);
            match done {
                Ok(()) => report.kept += 1,
                Err(err) => report.lost.push(lost(match err {
                    EditError::CannotAttach(AddNote::CarriesOthers) => LostWhy::CarriesOthers,
                    EditError::CannotAttach(_) => LostWhy::CannotAttach,
                    EditError::NothingThere => LostWhy::SurfaceChanged,
                    EditError::NoRoute(_) | EditError::Tessellation(_) | EditError::NotHeld => {
                        LostWhy::NoRoute
                    }
                    EditError::CannotBrace(why) => LostWhy::BraceRefused(why),
                })),
            }
        }
        // The braces last: a support placed by hand may hold one end, and a
        // brace of the run removed by hand may be one a placement lays.
        for unbraced in &log.unbraced {
            if cancelled() {
                return None;
            }
            if let Some(id) = session.run_brace_near(unbraced.from, unbraced.to) {
                let _ = session.remove_tips(&[id]);
            }
            report.kept += 1;
        }
        for braced in &log.braces {
            if cancelled() {
                return None;
            }
            let lost = |why| LostEdit {
                kind: HandEditKind::Braced,
                at: braced.from,
                why,
            };
            let ends = (
                session.brace_end_within(braced.from, MATCH_TIP_MM),
                session.brace_end_within(braced.to, MATCH_TIP_MM),
            );
            let (Some(mut from), Some(mut to)) = ends else {
                report.lost.push(lost(LostWhy::BraceEndsGone));
                continue;
            };
            // The heights it had, on the shafts there now.
            from.at[2] = braced.from[2];
            to.at[2] = braced.to[2];
            let mut laid = session.clone();
            match laid.brace_between(&from, &to) {
                Ok(_) => {
                    session = laid;
                    report.kept += 1;
                }
                Err((EditError::CannotBrace(why), _)) => {
                    report.lost.push(lost(LostWhy::BraceRefused(why)));
                }
                Err(_) => report.lost.push(lost(LostWhy::BraceEndsGone)),
            }
        }
        Some((session, report))
    }

    /// The contact diameter of a tip added to this set without a size, in
    /// millimetres: its preset's. A tip that hangs from a support gets just
    /// that; without zone sizes, one on a trunk of its own is sized from it
    /// by the surface it stands on, so it comes out close to it.
    pub fn tip_diameter_mm(&self) -> f64 {
        tip_contact_diameter(&self.settings)
    }

    /// How high this set's raft and the feet of its supports on it reach over
    /// the plate, in millimetres, as the mesh draws them: the raft's plate or
    /// lines and its wall, each root's disk and its cone. What the view from
    /// under the plate hides (#481); 0 with neither.
    pub fn raft_top_mm(&self) -> f64 {
        raft_and_feet_top_mm(&self.state.collections, &self.settings.rafts.global)
    }

    /// The tip diameter of support `id`, when it has one.
    fn tip_diameter(&self, id: &str) -> Option<f64> {
        let c = &self.state.collections;
        c.trunks
            .get(id)
            .and_then(|t| t.contact_cone.as_ref())
            .or_else(|| c.branches.get(id).and_then(|b| b.contact_cone.as_ref()))
            .or_else(|| c.leaves.get(id).map(|l| &l.contact_cone))
            .map(|k| k.profile.contact_diameter_mm)
    }

    /// The size (#360) support `id`'s tip was drawn with: one of the run's
    /// zone sizes, else one of the shipped ones; `None` for a tip no size
    /// draws (a run without sizes scales its tips by the surface).
    fn size_of(&self, id: &str) -> Option<String> {
        let d = self.tip_diameter(id)?;
        let same = |s: &&support_gen_df::supports::auto_support::zone_rules::SupportSize| {
            (s.tip_diameter_mm - d).abs() < 1e-6
        };
        self.settings
            .supports
            .auto_support
            .zone_rules
            .as_ref()
            .and_then(|r| r.sizes.iter().find(same).map(|s| s.id.clone()))
            .or_else(|| {
                crate::zone_presets::support_sizes()
                    .iter()
                    .find(same)
                    .map(|s| s.id.clone())
            })
    }

    /// The numbers support `id` was drawn with: the person's own for one
    /// placed by hand with them, else those of the size its tip has.
    pub fn shape_of(&self, id: &str) -> Option<SupportSize> {
        if let Some(shape) = self
            .hand_log
            .placed
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| p.shape.clone())
        {
            return Some(shape);
        }
        let size = self.size_of(id)?;
        self.settings
            .supports
            .auto_support
            .zone_rules
            .as_ref()
            .and_then(|r| r.sizes.iter().find(|s| s.id == size).cloned())
            .or_else(|| {
                crate::zone_presets::support_sizes()
                    .into_iter()
                    .find(|s| s.id == size)
            })
    }

    /// Draws support `tip` again with the numbers of `shape`, its tip where
    /// it is. A support others hang from stays as it is, as with a move. On
    /// a refusal the session is as it was.
    pub fn reshape(&mut self, tip: &str, shape: &SupportSize) -> Result<(), EditError> {
        let (at, _) = self.support_tip(tip).ok_or(EditError::NothingThere)?;
        let how = AddOptions {
            shape: Some(HandShape(shape.clone())),
            ..AddOptions::default()
        };
        self.move_tip(tip, at, &how)
    }

    /// The entities removing the supports `ids` takes, by the removal's
    /// cascade.
    fn removal_of(&self, ids: &[String]) -> Result<HashSet<String>, EditError> {
        let index = self.removal_index();
        let starts: Vec<usize> = ids
            .iter()
            .filter_map(|id| index.index_of.get(id).copied())
            .collect();
        if starts.is_empty() {
            return Err(EditError::NothingThere);
        }
        Ok(self.cascade(&starts))
    }

    /// Removes the supports `ids` (by their own ids, as [`Self::tips`]
    /// gives them) and everything that hangs from them, as one edit.
    /// Returns how many entities went.
    pub fn remove_tips(&mut self, ids: &[String]) -> Result<usize, EditError> {
        let gone = self.removal_of(ids)?;
        self.log_removal(ids, &gone);
        self.state.collections = filtered(&self.state.collections, |id| !gone.contains(id));
        self.removal_index = Arc::default();
        self.hand_edited = true;
        Ok(gone.len())
    }

    /// What [`Self::remove_tips`] would take away, drawn on its own.
    pub fn preview_remove_tips(&self, ids: &[String]) -> Result<EditPreview, EditError> {
        let gone = self.removal_of(ids)?;
        Ok(EditPreview {
            mesh: self.mesh_of(|id| gone.contains(id))?,
            valid: false,
            attached: false,
            note: None,
            taken: None,
            brace: None,
        })
    }

    /// Moves the tip of support `tip` to `to` (world mm) in place: the
    /// support goes, with what its removal takes (its root, its braces),
    /// and one is added at `to` as Add places it, with the size its tip had.
    /// A support others hang from stays. Returns what the move added and
    /// what it took.
    fn move_in_place(
        &mut self,
        tip: &str,
        to: At,
        how: &AddOptions,
    ) -> Result<(Placed, HashSet<String>), (EditError, Option<AddNote>)> {
        let gone = self
            .removal_of(&[tip.to_owned()])
            .map_err(|err| (err, None))?;
        let c = &self.state.collections;
        let carries = gone.iter().any(|id| {
            id != tip
                && (c.trunks.contains_key(id)
                    || c.branches.contains_key(id)
                    || c.leaves.contains_key(id)
                    || c.twigs.contains_key(id)
                    || c.sticks.contains_key(id)
                    || c.stumps.contains_key(id))
        });
        if carries {
            return Err((
                EditError::CannotAttach(AddNote::CarriesOthers),
                Some(AddNote::CarriesOthers),
            ));
        }
        let how = self.moved_how(tip, how);
        // The log (#408): a support of the run keeps where the run had it,
        // one placed by hand where it first came from.
        let moved_from = match self.hand_log.placed.iter().position(|p| p.id == tip) {
            Some(k) => self.hand_log.placed.remove(k).moved_from,
            None if is_hand_id(tip) => None,
            None => self.support_tip(tip).map(|(at, _)| at),
        };
        self.forget_braces(&gone);
        self.state.collections = filtered(&self.state.collections, |id| !gone.contains(id));
        self.removal_index = Arc::default();
        let placed = self.place_at(to, &how)?;
        if let Some(last) = self.hand_log.placed.last_mut()
            && placed.added.contains(&last.id)
        {
            last.moved_from = moved_from;
        }
        Ok((placed, gone))
    }

    /// Moves the tip of support `tip` (an id [`Self::tips`] gives) to `to`
    /// (world mm), the support routed again behind it the way Add places one
    /// (#3). On a refusal the session is as it was.
    pub fn move_tip(&mut self, tip: &str, to: [f64; 3], how: &AddOptions) -> Result<(), EditError> {
        let mut moved = self.clone();
        moved
            .move_in_place(tip, At::Point(to), how)
            .map_err(|(err, _)| err)?;
        *self = moved;
        Ok(())
    }

    /// What moving the tip of support `tip` to `to` would do: the support
    /// placed there (green, with what it leaves in `taken`), or red where
    /// nothing can hold that point or others hang from the support. With
    /// the session the move leaves when it goes through.
    /// How support `tip` is made again where it moves: as `how` says, else
    /// with its own size and shape.
    fn moved_how(&self, tip: &str, how: &AddOptions) -> AddOptions {
        AddOptions {
            attach: how.attach,
            size: how.size.clone().or_else(|| self.size_of(tip)),
            shape: how.shape.clone().or_else(|| {
                let placed = self.hand_log.placed.iter().find(|p| p.id == tip)?;
                placed.shape.clone().map(HandShape)
            }),
        }
    }

    pub fn preview_move_tip(
        &self,
        tip: &str,
        to: [f64; 3],
        how: &AddOptions,
    ) -> Result<(EditPreview, Option<Self>), EditError> {
        let mut copy = self.clone();
        match copy.move_in_place(tip, At::Point(to), how) {
            Ok((placed, gone)) => {
                let preview = EditPreview {
                    mesh: copy.mesh_of(|id| placed.added.contains(id))?,
                    valid: true,
                    attached: placed.attached,
                    note: placed.note,
                    taken: Some(self.mesh_of(|id| gone.contains(id))?),
                    brace: None,
                };
                Ok((preview, Some(copy)))
            }
            Err((_, Some(AddNote::CarriesOthers))) => {
                let gone = self.removal_of(&[tip.to_owned()])?;
                let preview = EditPreview {
                    mesh: self.mesh_of(|id| gone.contains(id))?,
                    valid: false,
                    attached: false,
                    note: Some(AddNote::CarriesOthers),
                    taken: None,
                    brace: None,
                };
                Ok((preview, None))
            }
            Err((EditError::NoRoute(_) | EditError::CannotAttach(_), note)) => {
                let preview = EditPreview {
                    mesh: self.refused_at(to, note, &self.moved_how(tip, how))?,
                    valid: false,
                    attached: false,
                    note,
                    taken: None,
                    brace: None,
                };
                Ok((preview, None))
            }
            Err((other, _)) => Err(other),
        }
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
        let picked: Vec<String> = self
            .nearest_support(point)
            .map(|(_, k)| self.removal_index().ids[k].clone())
            .into_iter()
            .collect();
        self.log_removal(&picked, &gone);
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
            taken: None,
            brace: None,
        })
    }

    /// Where brace `id` has its ends (world mm): its knots.
    fn brace_ends(&self, id: &str) -> Option<[[f64; 3]; 2]> {
        let c = &self.state.collections;
        let brace = c.braces.get(id)?;
        let at = |knot: &str| c.knots.get(knot).map(|k| [k.pos.x, k.pos.y, k.pos.z]);
        Some([at(&brace.start_knot_id)?, at(&brace.end_knot_id)?])
    }

    /// How far `point` lies off brace `brace`'s surface, 0 on it, when it
    /// lies on it: within [`ON_BRACE_MM`] of the brace's surface where it
    /// runs in the open, between the shafts it joins (#3).
    fn on_brace(
        &self,
        brace: &support_gen_df::supports::types::Brace,
        point: [f64; 3],
    ) -> Option<f64> {
        let c = &self.state.collections;
        let (s, e) = (
            c.knots.get(&brace.start_knot_id)?,
            c.knots.get(&brace.end_knot_id)?,
        );
        let (a, b) = ([s.pos.x, s.pos.y, s.pos.z], [e.pos.x, e.pos.y, e.pos.z]);
        let len = dist(a, b);
        // Each end sinks into its shaft by the shaft's radius.
        let (ra, rb) = (
            s.diameter.unwrap_or(0.0) / 2.0,
            e.diameter.unwrap_or(0.0) / 2.0,
        );
        if len <= ra + rb {
            return None;
        }
        let at = |t: f64| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
        let (from, to) = (at(ra / len), at(1.0 - rb / len));
        let radius = brace.profile.diameter / 2.0;
        let d = segment_dist(point, from, to);
        (d <= radius + ON_BRACE_MM).then(|| (d - radius).max(0.0))
    }

    /// Every brace with its id and its two ends (world mm): what a box or a
    /// brush finds it by on screen, as [`Self::support_paths`] does a
    /// support (#3).
    pub fn brace_paths(&self) -> Vec<(String, Vec<[f64; 3]>)> {
        self.state
            .collections
            .braces
            .keys()
            .filter_map(|id| Some((id.clone(), self.brace_ends(id)?.to_vec())))
            .collect()
    }

    /// Drops from the log the braces laid by hand among `gone` (#408): a
    /// support removed or moved takes its braces with it.
    fn forget_braces(&mut self, gone: &HashSet<String>) {
        self.hand_log.braces.retain(|b| !gone.contains(&b.id));
    }

    /// The brace of the run (not one laid by hand) whose ends lie within
    /// [`MATCH_TIP_MM`] of `from` and `to`, either way round.
    fn run_brace_near(&self, from: [f64; 3], to: [f64; 3]) -> Option<String> {
        self.state
            .collections
            .braces
            .keys()
            .filter(|id| !is_hand_id(id))
            .filter_map(|id| {
                let [a, b] = self.brace_ends(id)?;
                let d = (dist(a, from).max(dist(b, to))).min(dist(a, to).max(dist(b, from)));
                (d <= MATCH_TIP_MM).then(|| (d, id.clone()))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map(|(_, id)| id)
    }

    /// The end a brace laid by hand takes at `point` (world mm, on or near a
    /// support): the trunk or branch whose axis runs nearest, within reach
    /// of it, and the point of that axis nearest `point` (#3). `None` with no
    /// shaft in reach.
    pub fn brace_end_near(&self, point: [f64; 3]) -> Option<BraceEnd> {
        self.brace_end_within(point, BRACE_REACH_MM)
    }

    fn brace_end_within(&self, point: [f64; 3], reach_mm: f64) -> Option<BraceEnd> {
        let c = &self.state.collections;
        let mut best: Option<(f64, String, [f64; 3])> = None;
        for (id, path) in self.support_paths() {
            if !c.trunks.contains_key(&id) && !c.branches.contains_key(&id) {
                continue;
            }
            for w in path.windows(2) {
                let q = segment_nearest(point, w[0], w[1]);
                let d = dist(point, q);
                if d <= reach_mm && best.as_ref().is_none_or(|b| d < b.0) {
                    best = Some((d, id.clone(), q));
                }
            }
        }
        best.map(|(_, shaft, at)| BraceEnd { shaft, at })
    }

    /// The index of the model shaft `id` stands on.
    fn model_of_shaft(&self, id: &str) -> Option<usize> {
        let c = &self.state.collections;
        let model = c
            .trunks
            .get(id)
            .map(|t| &t.base.model_id)
            .or_else(|| c.branches.get(id).map(|b| &b.base.model_id))
            .or_else(|| c.kickstands.get(id).map(|k| &k.base.model_id))?;
        self.models.iter().position(|m| m.id == *model)
    }

    /// Lays a brace from `from` to `to` by the run's bracing rules and sizes
    /// (#3) and logs it (#408). Returns what it added; a refusal comes with
    /// the brace as it would be, to draw red, when both ends lie on their
    /// shafts.
    fn brace_between(
        &mut self,
        from: &BraceEnd,
        to: &BraceEnd,
    ) -> Result<HashSet<String>, (EditError, Option<Box<HandBrace>>)> {
        let Some(i) = self.model_of_shaft(&from.shaft) else {
            return Err((EditError::CannotBrace(HandBraceRefusal::NotAShaft), None));
        };
        let bvh = self.bvh(i);
        let meshes: BraceMeshes<'_> = [(
            self.models[i].id.clone(),
            BraceMesh {
                bvh: &bvh.bvh,
                matrix: bvh.to_world,
            },
        )]
        .into();
        let prefix = self.id_prefix.clone();
        let mut next = self.next_id;
        let mut new_id = || {
            next += 1;
            format!("{prefix}-m{next}")
        };
        let outcome = hand_brace(
            &self.state,
            (&from.shaft, from.at[2]),
            (&to.shaft, to.at[2]),
            &self.settings.supports.auto_bracing,
            &self.settings.supports,
            &meshes,
            &mut new_id,
        );
        let laid = match (outcome.brace, outcome.refusal) {
            (Some(laid), None) => laid,
            (laid, why) => {
                let why = why.unwrap_or(HandBraceRefusal::OffShaft);
                return Err((EditError::CannotBrace(why), laid.map(Box::new)));
            }
        };
        self.next_id = next;
        let [start, end] = laid.knots;
        let pos = |k: &Knot| [k.pos.x, k.pos.y, k.pos.z];
        self.hand_log.braces.push(HandBraced {
            id: laid.brace.base.id.clone(),
            model: laid.brace.base.model_id.clone(),
            from: pos(&start),
            to: pos(&end),
        });
        let added: HashSet<String> =
            [laid.brace.base.id.clone(), start.id.clone(), end.id.clone()].into();
        let c = &mut self.state.collections;
        c.knots.insert(start.id.clone(), start);
        c.knots.insert(end.id.clone(), end);
        c.braces.insert(laid.brace.base.id.clone(), laid.brace);
        self.removal_index = Arc::default();
        self.hand_edited = true;
        Ok(added)
    }

    /// Shaft `id` drawn on its own, with what it stands on or hangs from:
    /// a trunk's root, a branch's knot.
    fn shaft_mesh(&self, id: &str) -> Result<IndexedMesh, EditError> {
        let c = &self.state.collections;
        let under = c
            .trunks
            .get(id)
            .map(|t| t.root_id.clone())
            .or_else(|| c.branches.get(id).map(|b| b.parent_knot_id.clone()));
        self.mesh_of(|e| e == id || under.as_ref() == Some(e))
    }

    /// Lays a brace from the end `from` (one [`Self::brace_end_near`] gave)
    /// to the support at `to` (world mm), by the run's bracing rules and
    /// sizes, as one hand edit (#3). On a refusal the session is as it was.
    pub fn add_brace(&mut self, from: &BraceEnd, to: [f64; 3]) -> Result<(), EditError> {
        let to = self.brace_end_near(to).ok_or(EditError::NothingThere)?;
        let mut laid = self.clone();
        laid.brace_between(from, &to).map_err(|(err, _)| err)?;
        *self = laid;
        Ok(())
    }

    /// What [`Self::add_brace`] would lay, drawn on its own: green when it
    /// goes, red with why when not (the brace as it would be, or the support
    /// pointed at when no brace can be drawn), with the session it leaves
    /// when it goes. `NothingThere` with no support at `to`.
    pub fn preview_brace(
        &self,
        from: &BraceEnd,
        to: [f64; 3],
    ) -> Result<(EditPreview, Option<Self>), EditError> {
        let to = self.brace_end_near(to).ok_or(EditError::NothingThere)?;
        let mut copy = self.clone();
        match copy.brace_between(from, &to) {
            Ok(added) => {
                let preview = EditPreview {
                    mesh: copy.mesh_of(|id| added.contains(id))?,
                    valid: true,
                    attached: false,
                    note: None,
                    taken: None,
                    brace: None,
                };
                Ok((preview, Some(copy)))
            }
            Err((EditError::CannotBrace(why), laid)) => {
                let mesh = match laid {
                    Some(laid) => {
                        let laid = *laid;
                        let mut state = SupportState::default();
                        let c = &mut state.collections;
                        for knot in laid.knots {
                            c.knots.insert(knot.id.clone(), knot);
                        }
                        c.braces.insert(laid.brace.base.id.clone(), laid.brace);
                        self.draw_alone(&state)?
                    }
                    None => self.shaft_mesh(&to.shaft)?,
                };
                let preview = EditPreview {
                    mesh,
                    valid: false,
                    attached: false,
                    note: None,
                    taken: None,
                    brace: Some(why),
                };
                Ok((preview, None))
            }
            Err((other, _)) => Err(other),
        }
    }

    /// The support a first point of the brace tool at `point` takes (#3),
    /// drawn on its own, with the end it gives. `NothingThere` with no trunk
    /// or branch in reach.
    pub fn preview_brace_end(&self, point: [f64; 3]) -> Result<(EditPreview, BraceEnd), EditError> {
        let end = self.brace_end_near(point).ok_or(EditError::NothingThere)?;
        let preview = EditPreview {
            mesh: self.shaft_mesh(&end.shaft)?,
            valid: true,
            attached: false,
            note: None,
            taken: None,
            brace: None,
        };
        Ok((preview, end))
    }

    /// The entities `keep` names, tessellated without a raft. A preview
    /// draws a few pieces of a big set on every hover: only those are
    /// copied, and the models go without their geometry, which only a raft
    /// reads (a 1.9M-face model cost ~8 ms a preview, #3).
    fn mesh_of(&self, keep: impl Fn(&String) -> bool) -> Result<IndexedMesh, EditError> {
        self.draw_alone(&SupportState {
            collections: picked(&self.state.collections, keep),
            ..SupportState::default()
        })
    }

    /// `state`'s supports tessellated on their own, without a raft.
    fn draw_alone(&self, state: &SupportState) -> Result<IndexedMesh, EditError> {
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
        let sink = support_soup(state, &models, &settings)
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

    /// #163: the tips a nudge took for nine islands of Damian's
    /// FALTATORTUGA scene, whose exact point no trunk could reach: each
    /// landed on another face within reach (a wall a step below, a slope
    /// beside the island), so the support held nothing and the island was
    /// found again. None is the island's underside.
    #[test]
    fn a_nudge_holds_the_underside_aimed_at_not_a_face_beside_it() {
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        // (aimed tip, its normal, nudged tip, its normal)
        let off = [
            (
                v(-43.202, 1.972, 11.066),
                v(0.066, -0.051, -0.996),
                v(-43.262, 1.787, 10.858),
                v(0.301, 0.953, -0.023),
            ),
            (
                v(-35.422, 3.162, 14.188),
                v(0.091, -0.024, -0.996),
                v(-35.282, 3.187, 14.248),
                v(0.735, -0.307, -0.604),
            ),
            (
                v(-46.310, 8.346, 14.569),
                v(0.012, -0.033, -0.999),
                v(-46.337, 8.208, 14.664),
                v(0.178, -0.550, -0.816),
            ),
            (
                v(37.506, 5.498, 32.221),
                v(0.009, -0.234, -0.972),
                v(37.479, 5.506, 32.001),
                v(0.900, -0.435, -0.030),
            ),
            (
                v(-48.996, 1.129, 25.192),
                v(0.168, 0.015, -0.986),
                v(-48.885, 1.110, 25.010),
                v(-0.983, 0.162, -0.084),
            ),
            (
                v(-49.161, 0.263, 25.912),
                v(-0.424, -0.191, -0.885),
                v(-49.368, 0.240, 25.880),
                v(-0.097, -0.713, -0.694),
            ),
            (
                v(-47.517, 0.583, 27.070),
                v(-0.105, -0.150, -0.983),
                v(-47.650, 0.818, 26.975),
                v(-0.981, -0.195, -0.007),
            ),
            (
                v(41.335, 11.988, 37.141),
                v(-0.186, -0.263, -0.947),
                v(41.245, 11.964, 36.959),
                v(0.955, 0.283, -0.092),
            ),
            (
                v(-49.435, -1.303, 28.865),
                v(0.046, 0.032, -0.998),
                v(-49.249, -1.191, 28.720),
                v(-0.206, -0.767, -0.608),
            ),
        ];
        for (k, (aimed, aimed_n, tip, n)) in off.into_iter().enumerate() {
            assert!(
                !same_underside(aimed, aimed_n, tip, n, 0.3),
                "case {k}: {tip:?} is not the underside aimed at"
            );
        }
        // The same underside a step aside, or a little tilted, holds it.
        let down = v(0.0, 0.0, -1.0);
        let aimed = v(1.0, 2.0, 3.0);
        assert!(same_underside(aimed, down, v(1.2, 2.0, 3.0), down, 0.3));
        let tilted = v(0.17, 0.0, -0.985);
        assert!(same_underside(aimed, down, v(1.0, 2.2, 3.02), tilted, 0.3));
        // A parallel face a step below is another surface; one past the
        // reach, too far.
        assert!(!same_underside(aimed, down, v(1.1, 2.0, 2.8), down, 0.3));
        assert!(!same_underside(aimed, down, v(1.4, 2.0, 3.0), down, 0.3));
        // #405: the same face up to two layers higher holds what grew there;
        // three layers up, or a layer and a bit below, it is something else.
        assert!(same_underside(aimed, down, v(1.1, 2.0, 3.08), down, 0.3));
        assert!(!same_underside(aimed, down, v(1.1, 2.0, 3.12), down, 0.3));
        assert!(!same_underside(aimed, down, v(1.1, 2.0, 2.94), down, 0.3));
    }

    #[test]
    fn climbs_go_up_two_layers_after_the_nudges() {
        let points = climbs([1.0, 2.0, 3.0], 0.3);
        // The spot and three rings of eight, at two heights.
        assert_eq!(points.len(), 2 * (1 + 3 * 8));
        assert_eq!(points[0], [1.0, 2.0, 3.05]);
        assert!(points.iter().all(|p| p[2] > 3.0 && p[2] <= 3.1 + 1e-9));
        assert!(
            points.windows(2).all(|w| w[0][2] <= w[1][2] + 1e-12),
            "lowest first"
        );
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
            ..AddOptions::default()
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

    /// #481: a set tells how high its raft reaches, its plate and its wall,
    /// and 0 without a raft.
    #[test]
    fn a_set_tells_how_high_its_raft_reaches() {
        use crate::options::RaftMode;
        let with = |raft: RaftMode, thickness: f64, wall: bool| {
            let mut request = crate::DfSupportRequest::default();
            request.options.raft = raft;
            request.options.raft_thickness_mm = thickness;
            request.options.raft_wall = wall;
            bare_slab(12.0, request).raft_top_mm()
        };
        let wall = DEFAULT_RAFT_WALL_HEIGHT_MM;
        assert!((with(RaftMode::Solid, 0.5, true) - (0.5 + wall)).abs() < 1e-9);
        assert!((with(RaftMode::Solid, 1.2, false) - 1.2).abs() < 1e-9);
        assert!(with(RaftMode::Lines, 0.5, false) > 0.0);
        assert_eq!(with(RaftMode::Off, 0.5, true), 0.0);

        // The feet count too: a zone preset's join cone stands on the raft's
        // plate and reaches past its wall, the ring seen from under the plate.
        let mut request = crate::DfSupportRequest::default();
        request.options.zones = Some(crate::zone_presets::minis_30());
        let mut zoned = bare_slab(12.0, request);
        zoned.add_trunk_at([25.0, 5.0, 12.0]).expect("a trunk");
        let root = zoned
            .state
            .collections
            .roots
            .values()
            .next()
            .expect("its root");
        let join = root.join_cone_mm.expect("a join cone");
        let foot = root.transform.pos.z + root.disk_height.max(0.5) + join;
        let top = zoned.raft_top_mm();
        assert!((top - foot).abs() < 1e-9, "{top} {foot}");
        assert!(top > 0.5 + wall, "past the raft's wall");
        let mesh = zoned.mesh().unwrap().expect("drawn");
        assert!(
            mesh.vertices()
                .iter()
                .any(|v| (f64::from(v.as_array()[2]) - top).abs() < 1e-3),
            "the mesh reaches that height"
        );

        // A root's own cone, without a zone preset.
        let mut plain = bare_slab(12.0, crate::DfSupportRequest::default());
        plain.add_trunk_at([25.0, 5.0, 12.0]).expect("a trunk");
        let root = plain
            .state
            .collections
            .roots
            .values()
            .next()
            .expect("its root");
        let cone_top =
            root.transform.pos.z + root.disk_height.max(0.01) + root.cone_height.max(0.01);
        let top = plain.raft_top_mm();
        assert!(
            (top - cone_top.max(0.5 + wall)).abs() < 1e-9,
            "{top} {cone_top}"
        );
    }

    /// #481: nothing of the raft the mesh draws reaches past the height the
    /// view from under the plate hides, with every preset's feet on it: the
    /// raft is the soup's tail, past the pieces the supports alone draw.
    #[test]
    fn the_raft_drawn_stays_under_the_height_it_tells() {
        for zones in [
            None,
            Some(crate::zone_presets::minis_30()),
            Some(crate::zone_presets::figures_150()),
        ] {
            let mut request = crate::DfSupportRequest::default();
            request.options.zones = zones;
            let mut session = bare_slab(12.0, request);
            session.add_trunk_at([25.0, 5.0, 12.0]).expect("a trunk");
            session.add_trunk_at([10.0, 5.0, 12.0]).expect("another");
            let with = support_soup(&session.state, &session.models, &session.settings).unwrap();
            let mut off = (*session.settings).clone();
            off.rafts.global.bottom_mode = RaftBottomMode::Off;
            off.rafts.per_model.clear();
            let alone = support_soup(&session.state, &session.models, &off).unwrap();
            let raft_from = with.parts[alone.parts.len() - 1];
            assert!(raft_from < with.floats.len(), "a raft is drawn");
            let raft_top = with.floats[raft_from..]
                .iter()
                .skip(2)
                .step_by(3)
                .fold(f32::MIN, |a, z| a.max(*z));
            let told = session.raft_top_mm();
            assert!(
                f64::from(raft_top) <= told + 1e-4,
                "the raft reaches {raft_top}, past {told}"
            );
        }
    }

    /// #518: on a line raft the roots stand on small feet, a little wider
    /// than a line, not on their disks; and the feet stay under the height
    /// the view from under the plate hides.
    #[test]
    fn on_a_line_raft_the_roots_stand_on_feet_a_little_wider_than_a_line() {
        for zones in [None, Some(crate::zone_presets::minis_30())] {
            let mut request = crate::DfSupportRequest::default();
            request.options.raft = crate::options::RaftMode::Lines;
            request.options.zones = zones;
            let mut session = bare_slab(12.0, request);
            session.add_trunk_at([25.0, 5.0, 12.0]).expect("a trunk");
            session.add_trunk_at([10.0, 5.0, 12.0]).expect("another");
            let raft = session.settings.rafts.global;
            assert_eq!(raft.bottom_mode, RaftBottomMode::Line);
            let roots: Vec<_> = session.state.collections.roots.values().collect();
            let disk = roots.iter().map(|r| r.diameter * 0.5).fold(0.0, f64::max);
            let foot = raft.line_width_mm * 0.5 * 1.15;
            assert!(disk > foot, "the disk {disk} would be wider");
            let with = support_soup(&session.state, &session.models, &session.settings).unwrap();
            let mut off = (*session.settings).clone();
            off.rafts.global.bottom_mode = RaftBottomMode::Off;
            off.rafts.per_model.clear();
            let alone = support_soup(&session.state, &session.models, &off).unwrap();
            // The supports' own pieces, before the raft's: what they draw on
            // the lines reaches no farther from a root than the foot.
            let raft_from = with.parts[alone.parts.len() - 1];
            let mut low = 0;
            for p in with.floats[..raft_from].as_chunks::<3>().0 {
                if f64::from(p[2]) > raft.line_height_mm * 0.5 {
                    continue;
                }
                low += 1;
                let near = roots
                    .iter()
                    .map(|r| {
                        (f64::from(p[0]) - r.transform.pos.x)
                            .hypot(f64::from(p[1]) - r.transform.pos.y)
                    })
                    .fold(f64::MAX, f64::min);
                assert!(near <= foot + 1e-3, "{near} from its root, past {foot}");
            }
            assert!(low > 0, "the feet stand on the plate");
            assert!(session.raft_top_mm() >= raft.line_height_mm);
        }
    }

    /// The wall a raft gets by default, mm.
    const DEFAULT_RAFT_WALL_HEIGHT_MM: f64 =
        support_gen_df::supports::rafts::crenelated::raft_types::DEFAULT_RAFT_SETTINGS.wall_height;

    #[test]
    fn a_set_tells_the_diameter_of_the_tips_added_to_it() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        session.add_trunk_at([5.0, 5.0, 12.0]).expect("a trunk");
        let trunk = session.state.collections.trunks.values().next().unwrap();
        let cone = trunk.contact_cone.as_ref().unwrap();
        let drawn = cone.profile.contact_diameter_mm;
        let preset = session.tip_diameter_mm();
        assert!((drawn - preset).abs() <= 0.25 * preset, "{drawn} {preset}");
    }

    #[test]
    fn a_tip_goes_on_the_first_other_point_the_engine_holds() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        let away = [500.0, 500.0, 12.0];
        let used = session
            .add_tip_among(
                away,
                &[[600.0, 500.0, 12.0], [5.0, 5.0, 12.0]],
                None,
                &|_| true,
            )
            .expect("the second one holds");
        assert_eq!(used, [5.0, 5.0, 12.0]);
        assert_eq!(session.state.collections.trunks.len(), 1);
        assert!(session.hand_edited);
    }

    /// #433: a tip the check turns down is not placed, the next one is tried,
    /// and the session is as if the first had never been there.
    #[test]
    fn a_tip_that_does_not_hold_gives_way_to_the_next_point() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        let (first, second) = ([5.0, 5.0, 12.0], [15.0, 5.0, 12.0]);
        let used = session
            .add_tip_among(first, &[second], None, &|contacts| {
                contacts.iter().any(|contact| contact.surface[0] > 10.0)
            })
            .expect("the second one holds");
        assert_eq!(used, second);
        assert_eq!(session.state.collections.trunks.len(), 1);
    }

    /// #433: tips that went in and held nothing are a refusal of their own,
    /// and leave nothing behind.
    #[test]
    fn tips_that_hold_nothing_leave_the_session_as_it_was() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        let asked = std::cell::Cell::new(0);
        let err =
            session.add_tip_among([5.0, 5.0, 12.0], &[[15.0, 5.0, 12.0]], None, &|contacts| {
                asked.set(asked.get() + 1);
                assert!(!contacts.is_empty(), "the check reads the new tip");
                false
            });
        assert_eq!(err, Err(EditError::NotHeld));
        assert!(asked.get() >= 2, "every point was tried");
        assert!(session.state.collections.trunks.is_empty());
        assert!(!session.hand_edited);
    }

    /// #466: every tip ends in the model by its own radius, whatever made it,
    /// and the ball that closes it stands on the contact point.
    #[test]
    fn every_tip_goes_into_the_model_by_its_radius() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        session.add_trunk_at([5.0, 5.0, 12.0]).expect("a trunk");
        session
            .add_tip_among([15.0, 5.0, 12.0], &[], Some("extrasmall"), &|_| true)
            .expect("a small tip");
        session
            .add_tip_among([25.0, 5.0, 12.0], &[], Some("heavy"), &|_| true)
            .expect("a heavy tip");
        let depths = session.tip_depths();
        assert_eq!(depths.len(), 3);
        assert!(
            depths.iter().all(|(_, inside)| *inside),
            "ends in the model"
        );
        let pieces = session.pieces();
        let contacts: Vec<_> = pieces.iter().flat_map(|p| &p.contacts).collect();
        assert_eq!(contacts.len(), 3);
        for contact in contacts {
            assert_eq!(contact.ball, Some(contact.surface));
            let ratio = contact.inserted_mm / contact.tip_radius_mm;
            assert!((ratio - 1.0).abs() < 0.1, "{contact:?}");
        }
    }

    /// #445: what the app runs goes on to the next landing; the port's own
    /// defaults, which its parity fixtures read, do not.
    #[test]
    fn the_app_runs_every_landing_and_the_port_does_not() {
        let app = crate::settings_for(&crate::DfSupportRequest::default());
        assert!(app.supports.mesh_to_mesh.every_landing);
        assert!(app.supports.auto_bracing.exact_clearance);
        assert!(app.supports.auto_support.near_fans);
        let port = PipelineSettings::defaults(0.05);
        assert!(!port.supports.mesh_to_mesh.every_landing);
        assert!(!port.supports.auto_bracing.exact_clearance);
        assert!(!port.supports.auto_support.near_fans);
    }

    /// #466: a session reopened with settings of old draws round tips too, and
    /// the port's defaults do not.
    #[test]
    fn round_tips_hold_on_any_settings_of_a_session() {
        let port = PipelineSettings::defaults(0.05);
        assert!(!port.tip_drawing.round_tip);
        assert!(with_round_tips(port).tip_drawing.round_tip);
    }

    /// #444: a set can start from nothing: a session with no support takes
    /// its first tip, and draws.
    #[test]
    fn an_empty_session_takes_its_first_tip() {
        let scene = slab_scene(12.0);
        let model = crate::scene_model_leaves(&scene, None).unwrap()[0].0;
        let mut session =
            crate::empty_df_session(&scene, model, &crate::DfSupportRequest::default())
                .unwrap()
                .expect("a model");
        assert!(session.is_empty());
        session
            .add_tip_among([5.0, 5.0, 12.0], &[], None, &|_| true)
            .expect("the tip goes in");
        assert!(!session.is_empty());
        assert!(session.mesh().unwrap().is_some(), "it draws");
    }

    #[test]
    fn a_tip_with_nowhere_to_go_tells_why_of_its_point() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        let away = [500.0, 500.0, 12.0];
        let err = session.add_tip_among(away, &[[600.0, 500.0, 12.0]], None, &|_| true);
        assert!(matches!(err, Err(EditError::NothingThere)), "{err:?}");
        assert!(session.state.collections.trunks.is_empty());
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
                size: Some(size.to_owned()),
                ..AddOptions::default()
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
            for tri in piece.as_chunks::<9>().0 {
                let mut ids = [0u32; 3];
                for (k, p) in tri.as_chunks::<3>().0.iter().enumerate() {
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

    /// A slab with its run's supports, as a box or a brush finds them.
    fn supported_slab() -> DfSupportSession {
        crate::start_df_supports(&slab_scene(12.0), crate::DfSupportRequest::default())
            .unwrap()
            .join()
            .unwrap()
            .sets
            .remove(0)
            .session
    }

    #[test]
    fn several_supports_go_at_once_as_one_by_one() {
        let session = supported_slab();
        let tips = session.tips();
        assert!(tips.len() > 6, "{}", tips.len());
        let some: Vec<String> = tips.iter().step_by(3).map(|(id, _)| id.clone()).collect();
        // At once, and one tip after another: the same supports are left.
        let mut at_once = session.clone();
        let preview = at_once.preview_remove_tips(&some).expect("a preview");
        assert!(!preview.valid && !preview.mesh.faces().is_empty());
        at_once.remove_tips(&some).expect("they go");
        let mut in_turn = session.clone();
        for (id, tip) in tips.iter().step_by(3) {
            if in_turn.tips().iter().any(|(left, _)| left == id) {
                in_turn
                    .remove_near([tip[0], tip[1], tip[2] - 0.05])
                    .unwrap();
            }
        }
        assert_eq!(at_once.state, in_turn.state);
        assert!(at_once.hand_edited());
        // Nothing left behind: no knot or brace naming what went.
        let left = all_ids(&at_once.state.collections);
        for knot in at_once.state.collections.knots.values() {
            let host = &knot.parent_shaft_id;
            assert!(
                at_once
                    .state
                    .collections
                    .trunks
                    .values()
                    .any(|t| t.segments.iter().any(|g| &g.id == host))
                    || at_once
                        .state
                        .collections
                        .branches
                        .values()
                        .any(|b| b.segments.iter().any(|g| &g.id == host))
                    || left.contains(host),
                "a knot on {host}, gone"
            );
        }
        assert_eq!(
            at_once.remove_tips(&["nothing".to_owned()]),
            Err(EditError::NothingThere)
        );
    }

    #[test]
    fn a_tip_moves_and_its_support_is_routed_again() {
        let session = slab_with_a_trunk();
        let tip = session.tip_near([5.0, 5.0, 12.0], 1.0).expect("its tip");
        assert_eq!(session.tip_near([30.0, 5.0, 12.0], 1.0), None);
        let to = [30.0, 5.0, 12.0];
        let (preview, moved) = session
            .preview_move_tip(&tip, to, &AddOptions::default())
            .expect("a preview");
        assert!(preview.valid && preview.taken.is_some());
        let moved = moved.expect("the session the move leaves");
        let mut by_hand = session.clone();
        by_hand.move_tip(&tip, to, &AddOptions::default()).unwrap();
        assert_eq!(by_hand.state, moved.state, "the preview is the move");
        let tips = by_hand.tips();
        assert_eq!(tips.len(), 1);
        assert!(dist(tips[0].1, to) < 0.5, "{:?}", tips[0].1);
        assert!(
            by_hand.tip_near([5.0, 5.0, 12.0], 1.0).is_none(),
            "gone from there"
        );
        // Off the model: nothing moves.
        let mut kept = session.clone();
        assert!(
            kept.move_tip(&tip, [500.0, 5.0, 12.0], &AddOptions::default())
                .is_err()
        );
        assert_eq!(kept.state, session.state);
    }

    /// A support placed by hand with the person's own numbers is drawn with
    /// them, whatever the sizes of the run are; a move keeps them, and a
    /// reshape draws it again where it stands.
    #[test]
    fn a_support_takes_the_shape_handed_to_it_and_keeps_it() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        let mut shape = crate::zone_presets::support_sizes().remove(1);
        assert_eq!(shape.id, "small");
        shape.tip_diameter_mm = 0.33;
        shape.trunk_diameter_mm = 1.7;
        let how = AddOptions {
            shape: Some(HandShape(shape.clone())),
            ..AddOptions::default()
        };
        session.add_at([5.0, 5.0, 12.0], &how).unwrap();
        let tip = session.tip_near([5.0, 5.0, 12.0], 1.0).unwrap();
        assert_eq!(session.tip_diameter(&tip), Some(0.33));
        assert_eq!(session.shape_of(&tip), Some(shape.clone()));
        session
            .move_tip(&tip, [30.0, 5.0, 12.0], &AddOptions::default())
            .unwrap();
        let (id, _) = session.tips().remove(0);
        assert_eq!(session.shape_of(&id), Some(shape.clone()));
        shape.tip_diameter_mm = 0.5;
        session.reshape(&id, &shape).unwrap();
        let tips = session.tips();
        assert_eq!(tips.len(), 1);
        assert!(dist(tips[0].1, [30.0, 5.0, 12.0]) < 0.5);
        assert_eq!(session.tip_diameter(&tips[0].0), Some(0.5));
        assert_eq!(session.shape_of(&tips[0].0), Some(shape));
    }

    #[test]
    fn a_moved_tip_keeps_its_size_and_one_others_hang_from_stays() {
        let mut session = bare_slab(12.0, crate::DfSupportRequest::default());
        let large = AddOptions {
            size: Some("large".to_owned()),
            ..AddOptions::default()
        };
        session.add_at([5.0, 5.0, 12.0], &large).unwrap();
        let tip = session.tip_near([5.0, 5.0, 12.0], 1.0).unwrap();
        let mut moved = session.clone();
        moved
            .move_tip(&tip, [30.0, 5.0, 12.0], &AddOptions::default())
            .unwrap();
        let (id, _) = moved.tips().remove(0);
        assert_eq!(moved.tip_diameter(&id), Some(0.40));
        // A support hanging from it: the trunk cannot move from under it.
        session.add_trunk_at([11.0, 5.0, 12.0]).unwrap();
        assert_eq!(session.state.collections.knots.len(), 1, "it hangs");
        let (preview, kept) = session
            .preview_move_tip(&tip, [30.0, 5.0, 12.0], &AddOptions::default())
            .unwrap();
        assert!(!preview.valid && kept.is_none());
        assert_eq!(preview.note, Some(AddNote::CarriesOthers));
        let before = session.state.clone();
        assert!(
            session
                .move_tip(&tip, [30.0, 5.0, 12.0], &AddOptions::default())
                .is_err()
        );
        assert_eq!(session.state, before);
    }

    /// The contact tip of the support `edited` holds and `before` does not.
    fn new_tip(before: &DfSupportSession, edited: &DfSupportSession) -> [f64; 3] {
        let old: Vec<String> = before.tips().into_iter().map(|(id, _)| id).collect();
        edited
            .tips()
            .into_iter()
            .find(|(id, _)| !old.contains(id))
            .expect("a new tip")
            .1
    }

    /// A tip placed by hand lands on the very point picked (#3), or the
    /// placement is refused with why: never on another surface near it.
    #[test]
    fn a_hand_placed_tip_lands_on_the_point_picked() {
        let mut session = slab_with_a_wall(crate::DfSupportRequest::default());
        // On the underside, where Add and Move tip both put it.
        for at in [[14.3, 6.1, 25.0], [30.7, 2.2, 25.0]] {
            let before = session.clone();
            session.add_trunk_at(at).expect("held");
            let tip = new_tip(&before, &session);
            assert!(dist(tip, at) < 0.01, "{tip:?} for {at:?}");
        }
        let (id, from) = session.tips().pop().unwrap();
        let to = [40.4, 7.7, 25.0];
        let before = session.clone();
        session.move_tip(&id, to, &AddOptions::default()).unwrap();
        assert!(dist(new_tip(&before, &session), to) < 0.01);
        assert!(session.tip_near(from, 0.01).is_none());

        // On the wall: the tip goes there (an adaptive cone leans to hold a
        // wall), or nowhere. The surface search a run uses cast up from
        // below and took the slab's underside right above it, 7 mm off.
        let wall = [11.0, 5.0, 18.0];
        let (preview, edited) = session
            .preview_add_with(wall, &AddOptions::default())
            .expect("a preview");
        match edited {
            Some(edited) => {
                assert!(preview.valid);
                assert!(dist(new_tip(&session, &edited), wall) < 0.01);
            }
            None => assert!(!preview.valid && preview.note.is_some()),
        }
        // On top: it looks up.
        let (top, _) = session
            .preview_add_with([20.0, 5.0, 27.0], &AddOptions::default())
            .expect("a preview");
        assert!(!top.valid);
        assert_eq!(top.note, Some(AddNote::FacesUp));
        // No support was tried there, and none is drawn (#491).
        assert!(top.mesh.faces().is_empty());
    }

    /// One model of boxes, each `(min, max)` in world mm, run with no
    /// options and its supports then cleared.
    fn bare_boxes(boxes: &[([f32; 3], [f32; 3])]) -> DfSupportSession {
        bare_boxes_with(boxes, Default::default())
    }

    /// [`bare_boxes`], run as `request` says.
    fn bare_boxes_with(
        boxes: &[([f32; 3], [f32; 3])],
        request: crate::DfSupportRequest,
    ) -> DfSupportSession {
        let mut session = crate::start_df_supports(&boxes_scene(boxes), request)
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

    #[test]
    fn a_facet_that_looks_up_on_an_underside_takes_the_tip_of_the_underside() {
        // A shelf 0.2 mm wide and 0.03 mm thick hangs 0.03 mm under a slab:
        // its top, a facet looking up, lies inside the underside a tip there
        // covers, as the facets of a detailed sculpt do (#491).
        let mut session = bare_boxes(&[
            ([0.0, 0.0, 25.0], [50.0, 10.0, 27.0]),
            ([20.0, 4.9, 24.94], [20.2, 5.1, 24.97]),
        ]);
        let on_facet = [20.1, 5.0, 24.97];
        let (_, _, face) = session.contact(on_facet).unwrap();
        assert!(face.z > 0.99, "the facet looks up");
        let (preview, edited) = session
            .preview_add_with(on_facet, &AddOptions::default())
            .expect("a preview");
        assert!(preview.valid, "{:?}", preview.note);
        let before = session.clone();
        session.add_trunk_at(on_facet).expect("held");
        assert!(dist(new_tip(&before, &session), on_facet) < 0.01);
        assert!(edited.is_some());

        // A slab's top seen as a facet is no place for a tip all the same.
        let (top, _) = session
            .preview_add_with([30.0, 5.0, 27.0], &AddOptions::default())
            .expect("a preview");
        assert_eq!(top.note, Some(AddNote::FacesUp));
    }

    #[test]
    fn a_tip_no_way_reaches_draws_its_drop_to_the_model_below() {
        // A slab 0.3 mm over a block: nothing fits in the gap, and the red
        // preview stops on the block instead of running through it to the
        // plate as a trunk would (#491).
        let session = bare_boxes(&[
            ([0.0, 0.0, 25.0], [50.0, 10.0, 27.0]),
            ([15.0, 0.0, 0.0], [35.0, 10.0, 24.7]),
        ]);
        let (preview, edited) = session
            .preview_add_with([25.0, 5.0, 25.0], &AddOptions::default())
            .expect("a preview");
        assert!(!preview.valid && edited.is_none());
        assert!(!preview.mesh.faces().is_empty());
        let lowest = preview
            .mesh
            .vertices()
            .iter()
            .map(|v| v.as_array()[2])
            .fold(f32::INFINITY, f32::min);
        assert!(lowest > 24.0, "drawn down to {lowest}");

        // Beside the block, a red trunk still goes to the plate where one
        // would stand.
        let open = bare_boxes(&[([0.0, 0.0, 25.0], [50.0, 10.0, 27.0])]);
        assert!(
            open.straight_drop([25.0, 5.0, 25.0], &AddOptions::default())
                .is_ok_and(|mesh| { mesh.vertices().iter().any(|v| v.as_array()[2] < 1.0) })
        );
    }

    /// A support from the model to the model below (a stick) is its two
    /// tips and its whole trunk between them, as thick as its shape says
    /// (#564: the tube ran back on itself, a thin neck between two wide
    /// cones and a ball where the lower tip met the trunk). A span shorter
    /// than both tips has them meet halfway.
    #[test]
    fn a_support_from_model_to_model_keeps_its_trunk_whole() {
        // With no preset, and under a zone preset whose sizes the shape is
        // not one of (the Manual page's, under Miniatures).
        let minis = crate::DfSupportRequest {
            options: crate::DfSupportOptions {
                zones: Some(crate::zone_presets::minis_30()),
                ..Default::default()
            },
            ..Default::default()
        };
        for (gap, tip_length, request) in [
            (8.0f32, 2.3, crate::DfSupportRequest::default()),
            (6.0, 2.3, crate::DfSupportRequest::default()),
            (6.0, 4.0, crate::DfSupportRequest::default()),
            (8.0, 2.3, minis.clone()),
            (6.0, 2.3, minis.clone()),
        ] {
            let mut session = bare_boxes_with(
                &[
                    ([0.0, 0.0, 25.0], [50.0, 10.0, 27.0]),
                    ([15.0, 0.0, 0.0], [35.0, 10.0, 25.0 - gap]),
                ],
                request,
            );
            let mut shape = crate::zone_presets::support_sizes().remove(2);
            shape.trunk_diameter_mm = 2.9;
            shape.tip_diameter_mm = 0.3;
            shape.tip_length_mm = tip_length;
            let how = AddOptions {
                shape: Some(HandShape(shape)),
                ..AddOptions::default()
            };
            session.add_at([25.0, 5.0, 25.0], &how).expect("placed");
            let c = &session.state.collections;
            assert_eq!(c.sticks.len(), 1, "gap {gap}: a stick");
            let stick = c.sticks.values().next().unwrap();
            let (low, high) = (stick.contact_cone_a.pos, stick.contact_cone_b.pos);
            assert!(low.z < high.z);
            let axis = high.sub(low);
            let span = axis.length();
            let axis = axis.multiply_scalar(1.0 / span);
            // The tips as drawn: their own lengths, or their shares of a span
            // shorter than both.
            let (la, lb) = (
                stick.contact_cone_a.profile.length_mm,
                stick.contact_cone_b.profile.length_mm,
            );
            let fit = (span / (la + lb)).min(1.0);
            let (la, lb) = (la * fit, lb * fit);
            // No ring is wider than the cones reach where it stands (a
            // doubling tube drew joints there), and between the tips every
            // ring is the trunk's, none narrower.
            let (r_tip, trunk) = (0.15, 1.45);
            let mesh = session.mesh().unwrap().unwrap();
            for v in mesh.vertices() {
                let [x, y, z] = v.as_array().map(f64::from);
                let d = Vec3::new(x, y, z).sub(low);
                let t = d.dot(axis);
                let r = d.sub(axis.multiply_scalar(t)).length();
                let reach = (r_tip + (trunk - r_tip) * t / la)
                    .min(r_tip + (trunk - r_tip) * (span - t) / lb)
                    .min(trunk * 1.35);
                assert!(r < reach + 0.15, "gap {gap}: {r} wide at {t} of {span}");
                if t > la + 0.2 && t < span - lb - 0.2 {
                    assert!(r > 1.4, "gap {gap}: {r} wide at {t} of {span}");
                }
            }
        }
    }

    /// Two copies of one mesh, the second moved and turned about Z, share
    /// one model geometry and one BVH (#561), and a support placed on the
    /// turned one is the same as on the other, turned with it: a trunk to
    /// the plate and a stick to the model below.
    #[test]
    fn copies_of_a_model_share_its_geometry_and_place_alike() {
        use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
        use sc_scene::LeafPayload;
        let mut scene = boxes_scene(&[
            ([0.0, 0.0, 25.0], [30.0, 10.0, 27.0]),
            ([18.0, 0.0, 0.0], [30.0, 10.0, 19.0]),
        ]);
        let first = crate::scene_model_leaves(&scene, None).unwrap()[0].0;
        let mesh = scene.leaf_mesh_arc(first).unwrap().clone();
        let turn = AffineTransform3D::from_translation([70.0, 50.0, 0.0])
            .compose(&AffineTransform3D::from_rotation_z(0.9));
        let copy = scene
            .add_leaf(
                None,
                Transform3D::Affine(turn),
                LeafPayload::Mesh(mesh.clone()),
            )
            .unwrap();
        scene.add_root(copy).unwrap();
        let place = |p: [f64; 3]| {
            let v = Transform3D::Affine(turn)
                .apply_to_vertex(sc_mesh_core::Vertex::new(p.map(|c| c as f32)));
            v.as_array().map(f64::from)
        };

        // The model the copy is: its geometry is the first's, and where the
        // pipeline puts it is where the scene shows it.
        let models = crate::scene_model_leaves(&scene, None).unwrap();
        assert!(Arc::ptr_eq(&models[0].1.geometry, &models[1].1.geometry));
        let soup = support_gen_df::pipeline::world_soup(&models[1].1);
        let baked = sc_scene::bake_mesh_to_world(&mesh, &Transform3D::Affine(turn)).unwrap();
        for corner in soup.as_chunks::<3>().0 {
            let near = baked.vertices().iter().any(|v| {
                let b = v.as_array();
                (0..3).all(|k| (b[k] - corner[k]).abs() < 1e-3)
            });
            assert!(near, "{corner:?} is no corner of the copy");
        }

        let request = crate::DfSupportRequest::default();
        for (point, stick) in [([5.0, 5.0, 25.0], false), ([24.0, 5.0, 25.0], true)] {
            let mut a = crate::empty_df_session(&scene, first, &request)
                .unwrap()
                .unwrap();
            let mut b = crate::empty_df_session(&scene, copy, &request)
                .unwrap()
                .unwrap();
            a.add_at(point, &AddOptions::default())
                .expect("placed on the first");
            b.add_at(place(point), &AddOptions::default())
                .expect("placed on the copy");
            let (ca, cb) = (&a.state.collections, &b.state.collections);
            assert_eq!(ca.sticks.len() == 1, stick, "at {point:?}");
            assert_eq!(ca.trunks.len() == 1, !stick, "at {point:?}");
            assert_eq!(
                (
                    ca.trunks.len(),
                    ca.sticks.len(),
                    ca.twigs.len(),
                    ca.branches.len()
                ),
                (
                    cb.trunks.len(),
                    cb.sticks.len(),
                    cb.twigs.len(),
                    cb.branches.len()
                ),
                "at {point:?}"
            );
            let mut on_a: Vec<[f64; 3]> = a.tips().into_iter().map(|(_, p)| place(p)).collect();
            let mut on_b: Vec<[f64; 3]> = b.tips().into_iter().map(|(_, p)| p).collect();
            on_a.sort_by(|x, y| x.partial_cmp(y).unwrap());
            on_b.sort_by(|x, y| x.partial_cmp(y).unwrap());
            assert_eq!(on_a.len(), on_b.len());
            for (x, y) in on_a.iter().zip(&on_b) {
                assert!(dist(*x, *y) < 1e-3, "{x:?} against {y:?}");
            }
            let size = |s: &DfSupportSession| s.mesh().unwrap().map(|m| m.faces().len());
            assert_eq!(size(&a), size(&b), "at {point:?}");
            // One BVH for both.
            a.warm();
            b.warm();
            let bvh = |s: &DfSupportSession| {
                s.memory_parts()
                    .into_iter()
                    .find(|(part, _, _)| *part == "bvh")
                    .map(|(_, at, _)| at)
            };
            assert!(bvh(&a).is_some() && bvh(&a) == bvh(&b));
        }
    }

    /// A red support (no way to hold the point) is drawn of the size a
    /// click would place, the person's own shape (#564: it showed the
    /// run's size).
    #[test]
    fn a_red_support_is_drawn_of_the_shape_asked_for() {
        let open = bare_boxes(&[([0.0, 0.0, 25.0], [50.0, 10.0, 27.0])]);
        let widest = |how: &AddOptions| {
            let mesh = open.straight_drop([25.0, 5.0, 25.0], how).unwrap();
            mesh.vertices()
                .iter()
                .map(|v| v.as_array().map(f64::from))
                .filter(|[_, _, z]| (8.0..20.0).contains(z))
                .map(|[x, y, _]| ((x - 25.0).powi(2) + (y - 5.0).powi(2)).sqrt())
                .fold(0.0, f64::max)
        };
        let mut shape = crate::zone_presets::support_sizes().remove(1);
        shape.trunk_diameter_mm = 3.0;
        let thick = widest(&AddOptions {
            shape: Some(HandShape(shape)),
            ..AddOptions::default()
        });
        assert!((thick - 1.5).abs() < 0.05, "trunk radius {thick}");
        assert!(widest(&AddOptions::default()) < 1.0);
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

    /// Every tip of `session`, rounded to a micron and sorted: what two sets
    /// made the same way share whatever their ids.
    fn tip_places(session: &DfSupportSession) -> Vec<[i64; 3]> {
        let mut places: Vec<[i64; 3]> = session
            .tips()
            .into_iter()
            .map(|(_, p)| p.map(|c| (c * 1000.0).round() as i64))
            .collect();
        places.sort_unstable();
        places
    }

    /// The slab's supports with a run support removed, another moved by its
    /// tip and one added by hand, in that order.
    fn edited_slab() -> (DfSupportSession, [f64; 3], [f64; 3], [f64; 3]) {
        let mut session = supported_slab();
        let mut tips = session.tips();
        tips.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
        let (removed_id, removed) = tips[1].clone();
        session.remove_tips(&[removed_id]).expect("it goes");
        // The furthest tip that carries no other support: one that does
        // stays where it is.
        let mut left = session.tips();
        left.sort_by(|a, b| b.1[0].total_cmp(&a.1[0]));
        let aim = |from: [f64; 3]| [from[0] - 1.5, from[1] + 1.0, 12.0];
        let (moved_id, moved_from) = left
            .into_iter()
            .skip(1)
            .find(|(id, from)| {
                session
                    .clone()
                    .move_tip(id, aim(*from), &AddOptions::default())
                    .is_ok()
            })
            .expect("a tip that moves");
        let to = aim(moved_from);
        session
            .move_tip(&moved_id, to, &AddOptions::default())
            .expect("it moves");
        let added = [25.4, 2.6, 12.0];
        session
            .add_at(
                added,
                &AddOptions {
                    size: Some("large".to_owned()),
                    ..AddOptions::default()
                },
            )
            .expect("it is added");
        (session, removed, to, added)
    }

    /// #408: the hand edits of a set go back on the same model's supports
    /// made again: the support removed is removed, the one moved is moved,
    /// the one added is added with its size, and the result holds the tips
    /// the edited set held.
    #[test]
    fn hand_edits_go_back_on_a_run_made_again() {
        let (old, removed, to, added) = edited_slab();
        let log = old.hand_log();
        assert_eq!(log.removed.len(), 1, "{log:?}");
        assert_eq!(log.placed.len(), 2, "{log:?}");
        assert!(dist(log.removed[0].tip, removed) < 1e-9);
        assert!(log.placed[0].moved_from.is_some() && dist(log.placed[0].tip, to) < 0.05);
        assert_eq!(log.placed[1].size.as_deref(), Some("large"));
        let fresh = supported_slab();
        assert!(!fresh.hand_edited() && fresh.hand_log().is_empty());
        let (kept, report) = fresh
            .with_hand_edits_of(&old, &|| false)
            .expect("not cancelled");
        assert_eq!(report.kept, 3, "{report:?}");
        assert!(report.lost.is_empty() && !report.unrecorded, "{report:?}");
        assert_eq!(tip_places(&kept), tip_places(&old));
        assert!(kept.hand_edited());
        let id = kept.tip_near(added, 0.05).expect("the added support");
        assert_eq!(kept.size_of(&id).as_deref(), Some("large"));
        // Its log is the old one's: a later recalculation keeps them again.
        assert_eq!(kept.hand_log().removed.len(), 1);
        assert_eq!(kept.hand_log().placed.len(), 2);
        // Cancelled between two edits: nothing comes back.
        assert!(fresh.with_hand_edits_of(&old, &|| true).is_none());
    }

    /// #408: on a model whose surface moved, a support added there no
    /// longer fits and is reported with where it was; a removal with no
    /// support of the new run near its place counts as kept.
    #[test]
    fn an_edit_off_the_new_surface_is_reported() {
        let (old, _, _, added) = edited_slab();
        let fresh = crate::start_df_supports(&slab_scene(14.0), crate::DfSupportRequest::default())
            .unwrap()
            .join()
            .unwrap()
            .sets
            .remove(0)
            .session;
        let (kept, report) = fresh.with_hand_edits_of(&old, &|| false).unwrap();
        assert_eq!(report.lost.len(), 2, "{report:?}");
        assert_eq!(report.kept, 1, "the removal: {report:?}");
        assert!(
            report
                .lost
                .iter()
                .all(|l| l.why == crate::LostWhy::SurfaceChanged)
        );
        assert_eq!(report.lost[1].kind, crate::HandEditKind::Added);
        assert!(dist(report.lost[1].at, added) < 0.05);
        assert_eq!(report.lost[0].kind, crate::HandEditKind::Moved);
        assert_eq!(tip_places(&kept), tip_places(&fresh), "nothing changed");
    }

    /// #408: removing a support placed by hand takes it out of the log, and
    /// removing one of the run moved by hand logs its old place removed.
    #[test]
    fn removals_of_hand_placed_supports_keep_the_log_right() {
        let (mut session, _, to, added) = edited_slab();
        session
            .remove_near([added[0], added[1], added[2] - 0.05])
            .unwrap();
        assert_eq!(session.hand_log().placed.len(), 1);
        assert_eq!(session.hand_log().removed.len(), 1);
        let moved = session.tip_near(to, 0.1).expect("the moved support");
        let from = session.hand_log().placed[0].moved_from.unwrap();
        session.remove_tips(&[moved]).unwrap();
        assert!(session.hand_log().placed.is_empty());
        assert_eq!(session.hand_log().removed.len(), 2);
        assert!(dist(session.hand_log().removed[1].tip, from) < 1e-9);
    }

    /// #408: the log goes into a saved document and comes back, follows a
    /// move of the model in the plate plane, and a document of an older
    /// build, which kept none, reads as unrecorded: its supports placed by
    /// hand, known by their ids, are put back.
    #[test]
    fn the_log_is_saved_moves_with_the_model_and_an_older_set_still_keeps_its_additions() {
        let (old, _, _, added) = edited_slab();
        let scene = slab_scene(12.0);
        let bytes = old.to_document().expect("a document");
        let reopened = DfSupportSession::from_document(&scene, 2, &bytes).unwrap();
        assert_eq!(reopened.hand_log(), old.hand_log());
        let moved = old.moved_in_plane(0, [3.0, -2.0]);
        let shifted = moved.hand_log().placed[1].tip;
        assert!(dist(shifted, [added[0] + 3.0, added[1] - 2.0, added[2]]) < 1e-9);
        // An older build's document: no log.
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value.as_object_mut().unwrap().remove("handLog");
        let older = serde_json::to_vec(&value).unwrap();
        let legacy = DfSupportSession::from_document(&scene, 2, &older).unwrap();
        assert!(legacy.hand_log().unrecorded && legacy.hand_log().placed.is_empty());
        let (kept, report) = supported_slab()
            .with_hand_edits_of(&legacy, &|| false)
            .unwrap();
        assert!(report.unrecorded);
        assert_eq!(
            report.kept, 2,
            "the two supports placed by hand: {report:?}"
        );
        assert!(kept.tip_near(added, 0.05).is_some());
        // A set with no hand edits writes no log, as before.
        let plain = supported_slab().to_document().unwrap();
        assert!(!String::from_utf8(plain).unwrap().contains("handLog"));
    }

    /// Where trunk `id`'s axis is at height `z` (world mm).
    fn axis_at(session: &DfSupportSession, id: &str, z: f64) -> [f64; 3] {
        let (_, path) = session
            .support_paths()
            .into_iter()
            .find(|(t, _)| t == id)
            .expect("the trunk");
        path.windows(2)
            .find(|w| (w[1][2]..=w[0][2]).contains(&z))
            .map(|w| {
                let t = (w[0][2] - z) / (w[0][2] - w[1][2]);
                std::array::from_fn(|k| w[0][k] + (w[1][k] - w[0][k]) * t)
            })
            .expect("a height along it")
    }

    /// Where trunk `id`'s shaft ends below its tip: its highest joint.
    fn shaft_top(session: &DfSupportSession, id: &str) -> f64 {
        let (_, path) = session
            .support_paths()
            .into_iter()
            .find(|(t, _)| t == id)
            .expect("the trunk");
        path[1][2]
    }

    /// Two trunks of the slab's run whose roots stand `lo` to `hi` mm apart.
    fn trunk_pair(session: &DfSupportSession, lo: f64, hi: f64) -> (String, String) {
        let c = &session.state.collections;
        let root = |id: &String| {
            let p = c.roots[&c.trunks[id].root_id].transform.pos;
            [p.x, p.y]
        };
        let ids: Vec<&String> = c.trunks.keys().collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                let (p, q) = (root(a), root(b));
                let d = (p[0] - q[0]).hypot(p[1] - q[1]);
                if (lo..=hi).contains(&d) {
                    return ((*a).clone(), (*b).clone());
                }
            }
        }
        panic!("no two trunks {lo} to {hi} mm apart");
    }

    /// The brace `edited` holds that `before` does not, with its knots.
    fn new_brace<'a>(
        before: &DfSupportSession,
        edited: &'a DfSupportSession,
    ) -> (
        &'a support_gen_df::supports::types::Brace,
        [support_gen_df::three::Vec3; 2],
    ) {
        let c = &edited.state.collections;
        let brace = c
            .braces
            .iter()
            .find(|(id, _)| !before.state.collections.braces.contains_key(*id))
            .map(|(_, b)| b)
            .expect("a new brace");
        (
            brace,
            [
                c.knots[&brace.start_knot_id].pos,
                c.knots[&brace.end_knot_id].pos,
            ],
        )
    }

    /// #3: a brace laid by hand from one trunk to another starts where the
    /// first was pointed at, on its axis, and ends on the second at 45° (the
    /// height asked was flatter), at the run's brace size; it is the
    /// person's, and the log keeps it. The preview's session is the edit.
    #[test]
    fn a_brace_laid_by_hand_joins_two_trunks_at_the_runs_angle() {
        let session = supported_slab();
        let (a, b) = trunk_pair(&session, 3.0, 6.0);
        let from = session
            .brace_end_near(axis_at(&session, &a, 2.5))
            .expect("the first trunk");
        assert_eq!(from.shaft, a);
        let to = axis_at(&session, &b, 4.0);
        let (preview, edited) = session.preview_brace(&from, to).expect("a preview");
        assert!(preview.valid, "{:?}", preview.brace);
        assert!(!preview.mesh.faces().is_empty());
        let edited = edited.expect("the session it leaves");
        let (brace, [p, q]) = new_brace(&session, &edited);
        assert!(brace.base.generated_by.is_none() && is_hand_id(&brace.base.id));
        assert!((p.z - 2.5).abs() < 1e-6, "it starts where pointed: {p:?}");
        let span = (q.x - p.x).hypot(q.y - p.y);
        assert!(
            ((q.z - p.z) - span).abs() < 0.05,
            "45°: rise {} span {span}",
            q.z - p.z
        );
        let settings = &session.settings.supports.auto_bracing;
        assert_eq!(brace.profile.diameter, settings.brace_diameter_mm);
        assert!(edited.hand_edited());
        let log = &edited.hand_log().braces;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].id, brace.base.id);
        // The click makes what the preview showed.
        let mut clicked = session.clone();
        clicked.add_brace(&from, to).expect("it goes");
        assert_eq!(clicked.state.collections, edited.state.collections);
        // Asked steeper than 45°, it ends where asked.
        let high = axis_at(&session, &b, p.z + span + 1.0);
        let mut steep = session.clone();
        steep.add_brace(&from, high).expect("a steeper one goes");
        let (_, [_, end]) = new_brace(&session, &steep);
        assert!((end.z - high[2]).abs() < 1e-6, "{end:?} {high:?}");
    }

    /// #3: a brace by hand is refused, with why and drawn red, between a
    /// trunk and itself, between trunks farther apart than the settings let
    /// a brace span, from a point past where the trunk's tip starts, where
    /// 45° would end above the other trunk, and through the model.
    #[test]
    fn a_brace_by_hand_says_why_it_cannot_go() {
        let session = supported_slab();
        let refused = |from: &BraceEnd, to: [f64; 3]| {
            let (preview, edited) = session.preview_brace(from, to).expect("a preview");
            assert!(!preview.valid && edited.is_none());
            let mut copy = session.clone();
            assert_eq!(
                copy.add_brace(from, to),
                Err(EditError::CannotBrace(preview.brace.expect("why")))
            );
            assert_eq!(copy.state.collections, session.state.collections);
            preview.brace.unwrap()
        };
        let (a, b) = trunk_pair(&session, 3.0, 8.0);
        let from = session.brace_end_near(axis_at(&session, &a, 4.0)).unwrap();
        let same = axis_at(&session, &a, 8.0);
        assert_eq!(refused(&from, same), HandBraceRefusal::SameSupport);
        let (far_a, far_b) = trunk_pair(&session, 20.0, 60.0);
        let far = session
            .brace_end_near(axis_at(&session, &far_a, 3.0))
            .unwrap();
        let far_to = axis_at(&session, &far_b, 3.0);
        assert_eq!(refused(&far, far_to), HandBraceRefusal::TooLong);
        let to = axis_at(&session, &b, 4.0);
        let tip = session.support_tip(&a).unwrap().0;
        let past = session.brace_end_near(tip).unwrap();
        assert_eq!(refused(&past, to), HandBraceRefusal::OffShaft);
        let near_top = session
            .brace_end_near(axis_at(&session, &a, shaft_top(&session, &a) - 0.5))
            .unwrap();
        // Level with it on the other trunk: 45° up would end past its top.
        let level = axis_at(
            &session,
            &b,
            near_top.at[2].min(shaft_top(&session, &b) - 0.2),
        );
        assert_eq!(refused(&near_top, level), HandBraceRefusal::TooFlat);
        // Across the wall hanging from the slab of `slab_with_a_wall`.
        let mut walled = slab_with_a_wall(crate::DfSupportRequest::default());
        walled.add_trunk_at([14.0, 5.0, 25.0]).expect("a trunk");
        let left = walled.brace_end_near([5.0, 5.0, 12.5]).unwrap();
        let (preview, _) = walled.preview_brace(&left, [14.0, 5.0, 12.5]).unwrap();
        assert_eq!(preview.brace, Some(HandBraceRefusal::HitsModel));
        assert!(!preview.mesh.faces().is_empty(), "drawn red");
        // Nothing to brace to off every support.
        assert_eq!(
            session
                .preview_brace(&from, [25.0, 5.0, 40.0])
                .map(|(p, _)| p.valid),
            Err(EditError::NothingThere)
        );
    }

    /// #3: a brace laid by hand is removed by a click on it alone, and
    /// leaves the log; a brace of the run removed by a click is logged by
    /// its ends; a support removed takes its braces out of the log.
    #[test]
    fn a_brace_is_removed_on_its_own_and_the_log_follows() {
        let mut session = supported_slab();
        let (a, b) = trunk_pair(&session, 3.0, 6.0);
        let from = session.brace_end_near(axis_at(&session, &a, 2.5)).unwrap();
        let before = session.clone();
        session
            .add_brace(&from, axis_at(&session, &b, 4.0))
            .unwrap();
        let (brace, [p, q]) = new_brace(&before, &session);
        let id = brace.base.id.clone();
        let middle = [
            (p.x + q.x) / 2.0,
            (p.y + q.y) / 2.0 + 0.4,
            (p.z + q.z) / 2.0,
        ];
        let mut unbraced = session.clone();
        unbraced.remove_near(middle).expect("the brace");
        assert_eq!(
            unbraced.state.collections, before.state.collections,
            "the brace and its knots alone"
        );
        assert!(unbraced.hand_log().braces.is_empty());
        assert!(unbraced.hand_log().unbraced.is_empty(), "{id} was by hand");
        // A run's brace: logged by where its ends were.
        let run = before
            .state
            .collections
            .braces
            .keys()
            .find(|id| !is_hand_id(id))
            .expect("a brace of the run")
            .clone();
        let [s, e] = before.brace_ends(&run).unwrap();
        let mut gone = before.clone();
        gone.remove_tips(std::slice::from_ref(&run)).unwrap();
        assert!(!gone.state.collections.braces.contains_key(&run));
        assert_eq!(
            gone.state.collections.trunks,
            before.state.collections.trunks
        );
        assert_eq!(gone.hand_log().unbraced.len(), 1);
        assert_eq!(gone.hand_log().unbraced[0].from, s);
        assert_eq!(gone.hand_log().unbraced[0].to, e);
        // A trunk the hand brace joins, removed: the brace goes with it.
        let mut cut = session.clone();
        cut.remove_tips(std::slice::from_ref(&a)).unwrap();
        assert!(!cut.state.collections.braces.contains_key(&id));
        assert!(cut.hand_log().braces.is_empty());
    }

    /// #3, #408: braces laid and removed by hand go back on a run made
    /// again, go into a saved document and come back, and follow the model
    /// moved in the plate plane.
    #[test]
    fn braces_by_hand_are_kept_saved_and_moved() {
        let mut old = supported_slab();
        let (a, b) = trunk_pair(&old, 3.0, 6.0);
        let from = old.brace_end_near(axis_at(&old, &a, 2.5)).unwrap();
        let before = old.clone();
        old.add_brace(&from, axis_at(&old, &b, 4.0)).unwrap();
        let (_, [p, q]) = new_brace(&before, &old);
        let run = before
            .state
            .collections
            .braces
            .keys()
            .find(|id| !is_hand_id(id))
            .unwrap()
            .clone();
        let [s, e] = before.brace_ends(&run).unwrap();
        old.remove_tips(&[run]).unwrap();
        let fresh = supported_slab();
        let (kept, report) = fresh.with_hand_edits_of(&old, &|| false).unwrap();
        assert_eq!(report.kept, 2, "{report:?}");
        assert!(report.lost.is_empty(), "{report:?}");
        let ends: Vec<[[f64; 3]; 2]> = kept
            .state
            .collections
            .braces
            .keys()
            .filter_map(|id| kept.brace_ends(id))
            .collect();
        let has = |x: [f64; 3], y: [f64; 3]| {
            ends.iter()
                .any(|[m, n]| dist(*m, x).max(dist(*n, y)) < 1e-6)
        };
        assert!(has([p.x, p.y, p.z], [q.x, q.y, q.z]), "the hand brace");
        assert!(!has(s, e) && !has(e, s), "the run's brace stays removed");
        assert_eq!(kept.hand_log().braces.len(), 1);
        assert_eq!(kept.hand_log().unbraced.len(), 1);
        // Saved and opened again.
        let scene = slab_scene(12.0);
        let bytes = old.to_document().unwrap();
        let reopened = DfSupportSession::from_document(&scene, 2, &bytes).unwrap();
        assert_eq!(reopened.hand_log(), old.hand_log());
        assert_eq!(reopened.state.collections, old.state.collections);
        // Moved in the plane: the log's ends go along.
        let moved = old.moved_in_plane(0, [3.0, -2.0]);
        let log = &moved.hand_log().braces[0];
        assert!(dist(log.from, [p.x + 3.0, p.y - 2.0, p.z]) < 1e-9);
        assert!(
            dist(
                moved.hand_log().unbraced[0].to,
                [e[0] + 3.0, e[1] - 2.0, e[2]]
            ) < 1e-9
        );
        // On a model whose supports are gone from there, it is reported.
        let bare = bare_slab(12.0, crate::DfSupportRequest::default());
        let (_, report) = bare.with_hand_edits_of(&old, &|| false).unwrap();
        assert!(
            report
                .lost
                .iter()
                .any(|l| l.kind == crate::HandEditKind::Braced
                    && l.why == crate::LostWhy::BraceEndsGone),
            "{report:?}"
        );
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

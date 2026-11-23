//! A plate of models as one (#725): the supports of each model keep clear of
//! the models standing near it.
//!
//! A run makes a model's supports where nothing stands near it, alone or as
//! a copy of another; packed tight on a plate, some of them run into a
//! neighbour. [`crowding`] finds every support that comes within its radius
//! and [`NEIGHBOUR_CLEARANCE_MM`] of another model, and [`settle_crowded`]
//! takes those off, with whatever hangs from them, and places their contacts
//! again on a field that holds the neighbours too, grown by that clearance:
//! routed round them, and hanging from their trunks when the plate shares
//! them. Everything else of the model stays as it was.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bread_support_bracing::auto_brace::build_auto_braced_snapshot;
use bread_support_geom::bvh::{MeshBvh, Side};
use bread_support_geom::geom::{Box3, Matrix4, Vec3};
use bread_support_model::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use bread_support_model::bezier::get_bezier_point_at_t;
use bread_support_model::shafts::{EndpointHosts, ShaftEntity, resolve_segment_endpoints};
use bread_support_model::support_kinds::{SupportOriginId, SupportTypeId};
use bread_support_model::tips::{ContactCone, get_final_socket_position};
use bread_support_model::types::{Segment, SegmentShape, SupportCollections, SupportState};
use bread_support_placement::auto_support::auto_place::helpers::{
    PlanCtx, PlanMesh, sync_contact_cone_diameters_with,
};
use bread_support_placement::auto_support::auto_place::place::{PlaceArgs, place_one_candidate};
use bread_support_placement::auto_support::parameter_sizing::SizingBands;
use bread_support_placement::auto_support::types::{CandidatePoint, CandidateSource};
use bread_support_placement::placement_logic::pathfinding::sdf_cache::SdfCache;
use rayon::prelude::*;

use crate::PipelineSettings;

/// How far a support keeps from a model not its own past its radius, mm
/// (#725): no closer than this to a neighbour, so the two print apart.
pub const NEIGHBOUR_CLEARANCE_MM: f64 = 0.2;

/// How far round a model's box another model is taken for a neighbour, mm:
/// as far as its supports reach out sideways.
pub const NEIGHBOUR_REACH_MM: f64 = 12.0;

/// Along a support's line, the most between two points measured, mm.
const SAMPLE_STEP_MM: f64 = 0.2;

/// A model of the plate as the supports of the others keep clear of it: its
/// BVH and where it stands.
#[derive(Debug, Clone)]
pub struct PlateModel {
    /// The id its supports carry.
    pub id: String,
    pub bvh: Arc<MeshBvh>,
    /// From the BVH's frame to the world: a turn and a move.
    pub matrix: Matrix4,
    inverse: Matrix4,
    /// Its box in the world.
    pub bounds: Box3,
}

impl PlateModel {
    /// `bvh` placed by `matrix`; its box is the tree's own box placed so,
    /// which holds the model however it is turned (wider than it when
    /// turned about Z, never narrower).
    pub fn new(id: impl Into<String>, bvh: Arc<MeshBvh>, matrix: Matrix4) -> Self {
        let bounds = bvh.bounds().map_or(Box3::EMPTY, |b| {
            Box3 {
                min: Vec3::new(f64::from(b[0]), f64::from(b[1]), f64::from(b[2])),
                max: Vec3::new(f64::from(b[3]), f64::from(b[4]), f64::from(b[5])),
            }
            .apply_matrix4(&matrix)
        });
        PlateModel {
            id: id.into(),
            inverse: matrix.invert(),
            bvh,
            matrix,
            bounds,
        }
    }

    /// Whether its box comes within `reach` of `other` on the plate.
    pub fn near(&self, other: &Box3, reach: f64) -> bool {
        !(self.bounds.max.x + reach < other.min.x
            || other.max.x + reach < self.bounds.min.x
            || self.bounds.max.y + reach < other.min.y
            || other.max.y + reach < self.bounds.min.y)
    }

    /// How far a world point lies from its surface, when within `bound`.
    pub fn distance(&self, p: Vec3, bound: f64) -> Option<f64> {
        self.bvh
            .closest_point_to_point(p.apply_matrix4(&self.inverse), 0.0, bound)
            .map(|hit| hit.distance)
    }

    /// Whether its box grown by `by` meets the box from `a` to `b`.
    fn reaches(&self, lo: Vec3, hi: Vec3, by: f64) -> bool {
        !(hi.x < self.bounds.min.x - by
            || lo.x > self.bounds.max.x + by
            || hi.y < self.bounds.min.y - by
            || lo.y > self.bounds.max.y + by
            || hi.z < self.bounds.min.z - by
            || lo.z > self.bounds.max.z + by)
    }
}

/// The models of `plate` near `model` (#725): those whose box comes within
/// [`NEIGHBOUR_REACH_MM`] of its own, itself left out.
pub fn neighbours_of<'a>(model: &PlateModel, plate: &'a [PlateModel]) -> Vec<&'a PlateModel> {
    plate
        .iter()
        .filter(|m| !std::ptr::eq(*m, model) && m.near(&model.bounds, NEIGHBOUR_REACH_MM))
        .collect()
}

/// A stretch of a support as the plate check reads it: a capsule round the
/// line from `a` to `b`, owned by the entity `id`.
#[derive(Debug, Clone)]
struct Capsule {
    id: String,
    kind: SupportTypeId,
    a: Vec3,
    b: Vec3,
    radius: f64,
}

/// The segments of a shaft as capsules, curves sampled.
fn shaft_capsules(
    out: &mut Vec<Capsule>,
    shaft: ShaftEntity<'_>,
    hosts: EndpointHosts<'_>,
    owner: (&str, SupportTypeId),
) {
    for (i, seg) in shaft.segments().iter().enumerate() {
        let Some(ends) = resolve_segment_endpoints(shaft, seg, i, hosts) else {
            continue;
        };
        let radius = segment_radius(seg);
        match &seg.shape {
            SegmentShape::Bezier(c) => {
                let at = |t: f64| {
                    get_bezier_point_at_t(
                        ends.start,
                        c.control_point1,
                        c.control_point2,
                        ends.end,
                        t,
                    )
                };
                const STEPS: u32 = 8;
                for k in 0..STEPS {
                    let (t0, t1) = (
                        f64::from(k) / f64::from(STEPS),
                        f64::from(k + 1) / f64::from(STEPS),
                    );
                    push(out, owner, at(t0), at(t1), radius);
                }
            }
            SegmentShape::Straight => push(out, owner, ends.start, ends.end, radius),
        }
    }
}

/// A segment's radius: its own, or a wider joint's.
fn segment_radius(seg: &Segment) -> f64 {
    let joint =
        |j: &Option<bread_support_model::types::Joint>| j.as_ref().map_or(0.0, |j| j.diameter);
    seg.diameter
        .max(joint(&seg.top_joint))
        .max(joint(&seg.bottom_joint))
        / 2.0
}

fn push(out: &mut Vec<Capsule>, (id, kind): (&str, SupportTypeId), a: Vec3, b: Vec3, radius: f64) {
    if radius > 0.0 && a.x.is_finite() && b.x.is_finite() {
        out.push(Capsule {
            id: id.to_owned(),
            kind,
            a,
            b,
            radius,
        });
    }
}

/// A tip cone as a capsule from its socket to its contact.
fn cone_capsule(out: &mut Vec<Capsule>, cone: &ContactCone, owner: (&str, SupportTypeId)) {
    let radius = cone
        .profile
        .body_diameter_mm
        .max(cone.profile.contact_diameter_mm)
        / 2.0;
    push(
        out,
        owner,
        get_final_socket_position(cone),
        cone.pos,
        radius,
    );
}

/// Every stretch of the supports `keep` picks (by entity), as capsules: the
/// shafts, their roots, their tips, the knots on them, the braces.
fn capsules(c: &SupportCollections, keep: &dyn Fn(&str, &str) -> bool) -> Vec<Capsule> {
    let mut out = Vec::new();
    // Who owns each shaft id (a segment's, or the entity's own).
    let mut owner_of: HashMap<&str, (&str, SupportTypeId)> = HashMap::new();
    for (id, t) in &c.trunks {
        owner_of.insert(id, (id, SupportTypeId::Trunk));
        for s in &t.segments {
            owner_of.insert(&s.id, (id, SupportTypeId::Trunk));
        }
    }
    for (id, b) in &c.branches {
        owner_of.insert(id, (id, SupportTypeId::Branch));
        for s in &b.segments {
            owner_of.insert(&s.id, (id, SupportTypeId::Branch));
        }
    }
    for (id, s) in &c.sticks {
        owner_of.insert(id, (id, SupportTypeId::Stick));
        for seg in &s.segments {
            owner_of.insert(&seg.id, (id, SupportTypeId::Stick));
        }
    }
    for (id, t) in &c.trunks {
        if !keep(id, &t.base.model_id) {
            continue;
        }
        let owner = (id.as_str(), SupportTypeId::Trunk);
        let root = c.roots.get(&t.root_id);
        if let Some(r) = root {
            let top = bread_support_model::shafts::root_top(r);
            push(&mut out, owner, r.transform.pos, top, r.diameter / 2.0);
        }
        shaft_capsules(
            &mut out,
            ShaftEntity::Trunk(t),
            EndpointHosts {
                root,
                host_knot: None,
            },
            owner,
        );
        if let Some(cone) = &t.contact_cone {
            cone_capsule(&mut out, cone, owner);
        }
    }
    for (id, b) in &c.branches {
        if !keep(id, &b.base.model_id) {
            continue;
        }
        let owner = (id.as_str(), SupportTypeId::Branch);
        shaft_capsules(
            &mut out,
            ShaftEntity::Branch(b),
            EndpointHosts {
                root: None,
                host_knot: c.knots.get(&b.parent_knot_id),
            },
            owner,
        );
        if let Some(cone) = &b.contact_cone {
            cone_capsule(&mut out, cone, owner);
        }
    }
    for (id, l) in &c.leaves {
        if !keep(id, &l.base.model_id) {
            continue;
        }
        let owner = (id.as_str(), SupportTypeId::Leaf);
        if let Some(knot) = c.knots.get(&l.parent_knot_id) {
            let socket = get_final_socket_position(&l.contact_cone);
            let radius = l.contact_cone.profile.body_diameter_mm / 2.0;
            push(&mut out, owner, knot.pos, socket, radius);
        }
        cone_capsule(&mut out, &l.contact_cone, owner);
    }
    for (id, t) in &c.twigs {
        if !keep(id, &t.base.model_id) {
            continue;
        }
        shaft_capsules(
            &mut out,
            ShaftEntity::Twig(t),
            EndpointHosts::default(),
            (id, SupportTypeId::Twig),
        );
    }
    for (id, s) in &c.sticks {
        if !keep(id, &s.base.model_id) {
            continue;
        }
        let owner = (id.as_str(), SupportTypeId::Stick);
        shaft_capsules(
            &mut out,
            ShaftEntity::Stick(s),
            EndpointHosts::default(),
            owner,
        );
        cone_capsule(&mut out, &s.contact_cone_a, owner);
        cone_capsule(&mut out, &s.contact_cone_b, owner);
    }
    for (id, s) in &c.stumps {
        if !keep(id, &s.base.model_id) {
            continue;
        }
        let owner = (id.as_str(), SupportTypeId::Stump);
        let top = s.root_pos.add(Vec3::new(0.0, 0.0, s.root_height));
        push(
            &mut out,
            owner,
            s.root_pos,
            top,
            s.root_base_diameter.max(s.root_top_diameter) / 2.0,
        );
        shaft_capsules(
            &mut out,
            ShaftEntity::Stump(s),
            EndpointHosts::default(),
            owner,
        );
        cone_capsule(&mut out, &s.contact_cone, owner);
    }
    for (id, k) in &c.kickstands {
        if !keep(id, &k.base.model_id) {
            continue;
        }
        let owner = (id.as_str(), SupportTypeId::Kickstand);
        let root = c.roots.get(&k.root_id);
        if let Some(r) = root {
            let top = bread_support_model::shafts::root_top(r);
            push(&mut out, owner, r.transform.pos, top, r.diameter / 2.0);
        }
        shaft_capsules(
            &mut out,
            ShaftEntity::Kickstand(k),
            EndpointHosts {
                root,
                host_knot: c.knots.get(&k.host_knot_id),
            },
            owner,
        );
    }
    for (id, b) in &c.braces {
        if !keep(id, &b.base.model_id) {
            continue;
        }
        let (Some(from), Some(to)) = (c.knots.get(&b.start_knot_id), c.knots.get(&b.end_knot_id))
        else {
            continue;
        };
        let owner = (id.as_str(), SupportTypeId::Brace);
        let radius = b.profile.diameter / 2.0;
        match &b.curve {
            Some(curve) => {
                let c = &curve.curve;
                let at = |t: f64| {
                    get_bezier_point_at_t(from.pos, c.control_point1, c.control_point2, to.pos, t)
                };
                const STEPS: u32 = 8;
                for k in 0..STEPS {
                    let (t0, t1) = (
                        f64::from(k) / f64::from(STEPS),
                        f64::from(k + 1) / f64::from(STEPS),
                    );
                    push(&mut out, owner, at(t0), at(t1), radius);
                }
            }
            None => push(&mut out, owner, from.pos, to.pos, radius),
        }
    }
    // A knot is a ball on its shaft, owned by the shaft's support.
    for knot in c.knots.values() {
        let Some(&(id, kind)) = owner_of.get(knot.parent_shaft_id.as_str()) else {
            continue;
        };
        let model = match kind {
            SupportTypeId::Trunk => c.trunks.get(id).map(|t| t.base.model_id.as_str()),
            SupportTypeId::Branch => c.branches.get(id).map(|b| b.base.model_id.as_str()),
            _ => c.sticks.get(id).map(|s| s.base.model_id.as_str()),
        };
        if let (Some(model), Some(d)) = (model, knot.diameter)
            && keep(id, model)
        {
            push(&mut out, (id, kind), knot.pos, knot.pos, d / 2.0);
        }
    }
    out
}

/// A support that comes too near a model not its own (#725).
#[derive(Debug, Clone, PartialEq)]
pub struct Crowded {
    /// The entity, by id.
    pub id: String,
    pub kind: SupportTypeId,
    /// How far its surface keeps from the nearest such model, mm (0 or
    /// less: it touches it).
    pub gap_mm: f64,
}

/// The supports of `c` that `keep` picks (by entity id and model id) whose
/// surface comes within `margin` mm of a model of `others`, nearest first,
/// by entity. Each stretch is measured every [`SAMPLE_STEP_MM`] along its
/// line, against the models whose box it reaches.
pub fn crowding(
    c: &SupportCollections,
    keep: &dyn Fn(&str, &str) -> bool,
    others: &[&PlateModel],
    margin: f64,
) -> Vec<Crowded> {
    if others.is_empty() {
        return Vec::new();
    }
    let stretches = capsules(c, keep);
    let gaps: Vec<(usize, f64)> = stretches
        .par_iter()
        .enumerate()
        .filter_map(|(i, cap)| {
            let bound = cap.radius + margin;
            let lo = cap.a.min(cap.b);
            let hi = cap.a.max(cap.b);
            let near: Vec<&&PlateModel> =
                others.iter().filter(|m| m.reaches(lo, hi, bound)).collect();
            if near.is_empty() {
                return None;
            }
            let length = cap.b.sub(cap.a).length();
            let steps = (length / SAMPLE_STEP_MM.min(bound)).ceil().max(1.0) as usize;
            let mut least = f64::INFINITY;
            for k in 0..=steps {
                let p = cap.a.lerp(cap.b, k as f64 / steps as f64);
                for m in &near {
                    if let Some(d) = m.distance(p, bound) {
                        least = least.min(d - cap.radius);
                    }
                }
            }
            (least < margin).then_some((i, least))
        })
        .collect();
    let mut by_id: HashMap<&str, (SupportTypeId, f64)> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (i, gap) in gaps {
        let cap = &stretches[i];
        match by_id.get_mut(cap.id.as_str()) {
            Some(entry) => entry.1 = entry.1.min(gap),
            None => {
                by_id.insert(&cap.id, (cap.kind, gap));
                order.push(&cap.id);
            }
        }
    }
    let mut out: Vec<Crowded> = order
        .into_iter()
        .map(|id| Crowded {
            id: id.to_owned(),
            kind: by_id[id].0,
            gap_mm: by_id[id].1,
        })
        .collect();
    out.sort_by(|a, b| a.gap_mm.total_cmp(&b.gap_mm).then_with(|| a.id.cmp(&b.id)));
    out
}

/// A contact a support taken off touched the model at, to place again.
#[derive(Debug, Clone)]
struct Contact {
    id: String,
    pos: Vec3,
    normal: Vec3,
    tip_mm: Option<f64>,
    source: CandidateSource,
}

/// The candidate source a support placed from a contact came from, by the
/// origin it carries.
fn source_of(origin: Option<SupportOriginId>) -> CandidateSource {
    match origin {
        Some(SupportOriginId::Overhang | SupportOriginId::Standalone) => CandidateSource::Overhang,
        Some(SupportOriginId::Stabilization) => CandidateSource::Stabilization,
        Some(SupportOriginId::Reinforcement) => CandidateSource::Reinforcement,
        _ => CandidateSource::Voxel,
    }
}

fn cone_contact(id: &str, cone: &ContactCone, origin: Option<SupportOriginId>) -> Contact {
    Contact {
        id: id.to_owned(),
        pos: cone.pos,
        normal: cone.surface_normal.unwrap_or(cone.normal),
        tip_mm: Some(cone.profile.contact_diameter_mm),
        source: source_of(origin),
    }
}

/// `ids` taken off `c`, with everything that hangs from them (members on
/// their shafts, braces and kickstands on their knots, and so on up), and
/// the knots and roots left with nothing. Returns the contacts of the
/// supports taken that touched the model, and the ids taken.
fn take_off(c: &mut SupportCollections, ids: &HashSet<String>) -> (Vec<Contact>, HashSet<String>) {
    let mut out: HashSet<String> = ids.clone();
    // The shaft ids each support owns: its segments' and its own.
    let shafts_of = |c: &SupportCollections, id: &str| -> Vec<String> {
        let segs: Option<&Vec<Segment>> = c
            .trunks
            .get(id)
            .map(|t| &t.segments)
            .or_else(|| c.branches.get(id).map(|b| &b.segments))
            .or_else(|| c.sticks.get(id).map(|s| &s.segments));
        let mut all: Vec<String> = segs.into_iter().flatten().map(|s| s.id.clone()).collect();
        all.push(id.to_owned());
        all
    };
    loop {
        let shafts: HashSet<String> = out.iter().flat_map(|id| shafts_of(c, id)).collect();
        let dead_knots: HashSet<&str> = c
            .knots
            .iter()
            .filter(|(_, k)| shafts.contains(&k.parent_shaft_id))
            .map(|(id, _)| id.as_str())
            .collect();
        let mut more: Vec<String> = Vec::new();
        for (id, b) in &c.branches {
            if !out.contains(id) && dead_knots.contains(b.parent_knot_id.as_str()) {
                more.push(id.clone());
            }
        }
        for (id, l) in &c.leaves {
            if !out.contains(id) && dead_knots.contains(l.parent_knot_id.as_str()) {
                more.push(id.clone());
            }
        }
        for (id, b) in &c.braces {
            if !out.contains(id)
                && (dead_knots.contains(b.start_knot_id.as_str())
                    || dead_knots.contains(b.end_knot_id.as_str()))
            {
                more.push(id.clone());
            }
        }
        for (id, k) in &c.kickstands {
            if !out.contains(id) && dead_knots.contains(k.host_knot_id.as_str()) {
                more.push(id.clone());
            }
        }
        if more.is_empty() {
            break;
        }
        out.extend(more);
    }
    let mut contacts = Vec::new();
    for (id, t) in &c.trunks {
        if out.contains(id)
            && let Some(cone) = &t.contact_cone
        {
            contacts.push(cone_contact(id, cone, t.base.origin));
        }
    }
    for (id, b) in &c.branches {
        if out.contains(id)
            && let Some(cone) = &b.contact_cone
        {
            contacts.push(cone_contact(id, cone, b.base.origin));
        }
    }
    for (id, l) in &c.leaves {
        if out.contains(id) {
            contacts.push(cone_contact(id, &l.contact_cone, l.base.origin));
        }
    }
    for (id, s) in &c.stumps {
        if out.contains(id) {
            contacts.push(cone_contact(id, &s.contact_cone, s.base.origin));
        }
    }
    for (id, s) in &c.sticks {
        if out.contains(id) {
            let upper = if s.contact_cone_a.pos.z >= s.contact_cone_b.pos.z {
                &s.contact_cone_a
            } else {
                &s.contact_cone_b
            };
            contacts.push(cone_contact(id, upper, s.base.origin));
        }
    }
    for (id, t) in &c.twigs {
        if out.contains(id) {
            let upper = if t.contact_disk_a.pos.z >= t.contact_disk_b.pos.z {
                &t.contact_disk_a
            } else {
                &t.contact_disk_b
            };
            contacts.push(Contact {
                id: id.clone(),
                pos: upper.pos,
                normal: upper.surface_normal,
                tip_mm: None,
                source: source_of(t.base.origin),
            });
        }
    }
    // What is taken goes, its knots with it.
    let shafts: HashSet<String> = out.iter().flat_map(|id| shafts_of(c, id)).collect();
    let roots_taken: HashSet<String> = c
        .trunks
        .iter()
        .filter(|(id, _)| out.contains(*id))
        .map(|(_, t)| t.root_id.clone())
        .chain(
            c.kickstands
                .iter()
                .filter(|(id, _)| out.contains(*id))
                .map(|(_, k)| k.root_id.clone()),
        )
        .collect();
    let named_before: HashSet<String> = knots_named(c, &|id| out.contains(id));
    c.trunks.retain(|id, _| !out.contains(id));
    c.branches.retain(|id, _| !out.contains(id));
    c.leaves.retain(|id, _| !out.contains(id));
    c.twigs.retain(|id, _| !out.contains(id));
    c.sticks.retain(|id, _| !out.contains(id));
    c.braces.retain(|id, _| !out.contains(id));
    c.stumps.retain(|id, _| !out.contains(id));
    c.kickstands.retain(|id, _| !out.contains(id));
    let still: HashSet<String> = knots_named(c, &|_| true);
    c.knots.retain(|id, k| {
        !shafts.contains(&k.parent_shaft_id) && (still.contains(id) || !named_before.contains(id))
    });
    let standing: HashSet<&str> = c
        .trunks
        .values()
        .map(|t| t.root_id.as_str())
        .chain(c.kickstands.values().map(|k| k.root_id.as_str()))
        .collect();
    c.roots
        .retain(|id, _| !roots_taken.contains(id) || standing.contains(id.as_str()));
    (contacts, out)
}

/// The knots the supports `pick` takes (by id) hang from or brace to.
fn knots_named(c: &SupportCollections, pick: &dyn Fn(&str) -> bool) -> HashSet<String> {
    let mut ids = HashSet::new();
    for (id, b) in &c.branches {
        if pick(id) {
            ids.insert(b.parent_knot_id.clone());
        }
    }
    for (id, l) in &c.leaves {
        if pick(id) {
            ids.insert(l.parent_knot_id.clone());
        }
    }
    for (id, b) in &c.braces {
        if pick(id) {
            ids.insert(b.start_knot_id.clone());
            ids.insert(b.end_knot_id.clone());
        }
    }
    for (id, k) in &c.kickstands {
        if pick(id) {
            ids.insert(k.host_knot_id.clone());
        }
    }
    ids
}

/// `c` without the supports `ids` names and what hangs from them (#725):
/// what a neighbour offers to hang from once its own crowded supports are
/// taken off.
pub fn without(c: &SupportCollections, ids: &HashSet<String>) -> SupportCollections {
    let mut c = c.clone();
    take_off(&mut c, ids);
    c
}

/// The supports a neighbour offers to hang from (#725): its trunks, roots,
/// members and knots, each id and model id ending in `suffix`, so they keep
/// apart from those of a model they were copied from.
pub fn as_hosts(c: &SupportCollections, suffix: &str) -> SupportCollections {
    let s = |id: &str| format!("{id}{suffix}");
    let mut out = SupportCollections::default();
    for (id, r) in &c.roots {
        let mut r = r.clone();
        r.base.id = s(&r.base.id);
        r.base.model_id = s(&r.base.model_id);
        out.roots.insert(s(id), r);
    }
    let segments = |segs: &[Segment]| -> Vec<Segment> {
        segs.iter()
            .map(|seg| {
                let mut seg = seg.clone();
                seg.id = s(&seg.id);
                if let Some(j) = &mut seg.top_joint {
                    j.id = s(&j.id);
                }
                if let Some(j) = &mut seg.bottom_joint {
                    j.id = s(&j.id);
                }
                seg
            })
            .collect()
    };
    for (id, t) in &c.trunks {
        let mut t = t.clone();
        t.base.id = s(&t.base.id);
        t.base.model_id = s(&t.base.model_id);
        t.root_id = s(&t.root_id);
        t.segments = segments(&t.segments);
        if let Some(cone) = &mut t.contact_cone {
            cone.id = s(&cone.id);
        }
        out.trunks.insert(s(id), t);
    }
    for (id, b) in &c.branches {
        let mut b = b.clone();
        b.base.id = s(&b.base.id);
        b.base.model_id = s(&b.base.model_id);
        b.parent_knot_id = s(&b.parent_knot_id);
        b.segments = segments(&b.segments);
        if let Some(cone) = &mut b.contact_cone {
            cone.id = s(&cone.id);
        }
        out.branches.insert(s(id), b);
    }
    for (id, l) in &c.leaves {
        let mut l = l.clone();
        l.base.id = s(&l.base.id);
        l.base.model_id = s(&l.base.model_id);
        l.parent_knot_id = s(&l.parent_knot_id);
        l.contact_cone.id = s(&l.contact_cone.id);
        out.leaves.insert(s(id), l);
    }
    for (id, k) in &c.knots {
        let mut k = k.clone();
        k.id = s(&k.id);
        k.parent_shaft_id = s(&k.parent_shaft_id);
        out.knots.insert(s(id), k);
    }
    out
}

/// What [`settle_crowded`] reads.
#[derive(Debug)]
pub struct SettleInput<'a> {
    /// The model whose supports are settled.
    pub model: &'a PlateModel,
    /// The models standing near it.
    pub others: &'a [&'a PlateModel],
    /// Its supports, every one carrying `model.id`.
    pub state: &'a SupportState,
    /// The neighbours' supports it may hang from ([`as_hosts`]); none to
    /// keep to its own.
    pub hosts: Option<&'a SupportCollections>,
    /// Its supports to take off and place again ([`crowding`]).
    pub crowded: &'a HashSet<String>,
    pub settings: &'a PipelineSettings,
}

/// What [`settle_crowded`] did.
#[derive(Debug, Clone)]
pub struct Settled {
    /// The model's supports, settled: its own only.
    pub state: SupportState,
    /// Supports taken off (the crowded and what hung from them).
    pub taken: usize,
    /// Contacts they touched the model at, placed again.
    pub placed: usize,
    /// Contacts left without a support: no way to them keeps clear of the
    /// neighbours.
    pub lost: Vec<Vec3>,
    /// Supports placed again hanging from a neighbour's trunk.
    pub on_neighbours: usize,
    /// Where the time went, ms: the field, placing, bracing again.
    pub field_ms: f64,
    pub place_ms: f64,
    pub brace_ms: f64,
    /// The engine's own timings of it (router, scans), when the thread was
    /// not timing something else.
    pub timings: Vec<(String, f64)>,
}

/// Takes `input.crowded` off the model's supports, with what hangs from them,
/// and places their contacts again routed round the neighbours (#725), a
/// contact whose new support still comes too near one tried once more on a
/// trunk of its own; the new trunks braced to the model's own. Ids come from
/// `new_id`.
pub fn settle_crowded(input: &SettleInput<'_>, new_id: &mut dyn FnMut() -> String) -> Settled {
    let timing = !bread_support_geom::perf::collecting();
    if timing {
        bread_support_geom::perf::begin_frame();
    }
    let model_id = input.model.id.as_str();
    let mut draft = input.state.clone();
    let (mut contacts, taken) = take_off(&mut draft.collections, input.crowded);
    if let Some(hosts) = input.hosts {
        let c = &mut draft.collections;
        c.roots.extend(hosts.roots.clone());
        c.trunks.extend(hosts.trunks.clone());
        c.branches.extend(hosts.branches.clone());
        c.leaves.extend(hosts.leaves.clone());
        c.knots.extend(hosts.knots.clone());
    }
    // Highest first: a lower contact may hang from a trunk raised for one
    // above it.
    contacts.sort_by(|a, b| b.pos.z.total_cmp(&a.pos.z).then_with(|| a.id.cmp(&b.id)));

    let settings = input.settings;
    let mut supports = settings.supports.clone();
    supports.auto_support.share_hosts = input.hosts.is_some();
    let bands = SizingBands::factory().with_zone_sizes(&supports.auto_support.zone_rules);
    let clock = std::time::Instant::now();
    let sdf = RefCell::new(
        SdfCache::new(
            input.model.bvh.clone(),
            input.model.matrix,
            Some(settings.sdf_cell_mm),
        )
        .with_placed_obstacles(
            input
                .others
                .iter()
                .map(|o| (o.bvh.clone(), o.matrix, o.bounds)),
            NEIGHBOUR_CLEARANCE_MM,
        )
        .without_column_maps(),
    );
    let field_ms = clock.elapsed().as_secs_f64() * 1000.0;
    let clock = std::time::Instant::now();
    let before: HashSet<String> = entity_ids(&draft.collections);
    let mut placed = 0;
    let mut lost = Vec::new();
    {
        let ctx = PlanCtx::new(
            &supports,
            &bands,
            Some(PlanMesh {
                bvh: &input.model.bvh,
                matrix: input.model.matrix,
                side: Side::Front,
                normals: None,
                sdf: &sdf,
                blocked: None,
            }),
            new_id,
        );
        let no_grid = HashSet::new();
        let place = |draft: &mut SupportState, contact: &Contact, own: bool| -> Option<String> {
            let zone = contact.tip_mm.and_then(|d| ctx.zone_of_tip(d));
            let candidate = CandidatePoint {
                id: format!("plate-{}", contact.id),
                tip_pos: contact.pos,
                tip_normal: contact.normal,
                model_id: model_id.to_owned(),
                // A trunk of its own hangs from nothing and takes no member.
                source: if own {
                    CandidateSource::Stabilization
                } else {
                    contact.source
                },
                island_area_mm2: 0.3,
                z_height: contact.pos.z,
                priority: 0.0,
                tip_diameter_mm: None,
                grid_point: None,
                zone_size: zone,
                enclosed: false,
            };
            ctx.set_zone(zone);
            ctx.set_exact_contact(Some((contact.pos, contact.normal)));
            let out = place_one_candidate(
                &candidate,
                draft,
                &PlaceArgs {
                    grid_host_ids: Some(&no_grid),
                },
                &ctx,
                true,
            );
            ctx.set_exact_contact(None);
            ctx.set_zone(None);
            out.entity_id
        };
        let others = input.others;
        let still_crowded = |draft: &SupportState, id: &str| -> bool {
            !crowding(
                &draft.collections,
                &|e, _| e == id,
                others,
                NEIGHBOUR_CLEARANCE_MM,
            )
            .is_empty()
        };
        for contact in &contacts {
            let mut done = false;
            for own in [false, true] {
                let Some(id) = place(&mut draft, contact, own) else {
                    continue;
                };
                if still_crowded(&draft, &id) {
                    take_off(&mut draft.collections, &HashSet::from([id]));
                    continue;
                }
                done = true;
                break;
            }
            if done {
                placed += 1;
            } else {
                lost.push(contact.pos);
            }
        }
    }
    draft = sync_contact_cone_diameters_with(&draft, supports.auto_support.member_host_shaft_ratio);
    // What hangs from a neighbour: its knots stay, the neighbour's own
    // supports go back.
    let on_neighbours = if input.hosts.is_some() {
        let c = &draft.collections;
        let foreign: HashSet<&str> = c
            .trunks
            .iter()
            .filter(|(_, t)| t.base.model_id != model_id)
            .flat_map(|(id, t)| {
                std::iter::once(id.as_str()).chain(t.segments.iter().map(|s| s.id.as_str()))
            })
            .collect();
        c.branches
            .iter()
            .filter(|(id, b)| !before.contains(*id) && b.base.model_id == model_id)
            .map(|(_, b)| &b.parent_knot_id)
            .chain(
                c.leaves
                    .iter()
                    .filter(|(id, l)| !before.contains(*id) && l.base.model_id == model_id)
                    .map(|(_, l)| &l.parent_knot_id),
            )
            .filter(|k| {
                c.knots
                    .get(*k)
                    .is_some_and(|k| foreign.contains(k.parent_shaft_id.as_str()))
            })
            .count()
    } else {
        0
    };
    if input.hosts.is_some() {
        own_supports(&mut draft.collections, model_id);
    }
    let place_ms = clock.elapsed().as_secs_f64() * 1000.0;
    let clock = std::time::Instant::now();
    // Braced again as a run braces, its braces and kickstands clear of
    // every model: the shafts are not those the run braced.
    if !supports.auto_support.debug_skip_auto_bracing {
        let mut meshes: BraceMeshes<'_> = BraceMeshes::default();
        meshes.insert(
            model_id.to_owned(),
            BraceMesh {
                bvh: &input.model.bvh,
                matrix: input.model.matrix,
                grown_mm: 0.0,
            },
        );
        for (k, o) in input.others.iter().enumerate() {
            meshes.insert(
                format!("{}#{k}", o.id),
                BraceMesh {
                    bvh: &o.bvh,
                    matrix: o.matrix,
                    grown_mm: NEIGHBOUR_CLEARANCE_MM,
                },
            );
        }
        let store = draft.clone();
        draft = build_auto_braced_snapshot(
            &draft,
            &supports.auto_bracing,
            &supports,
            &meshes,
            &store,
            new_id,
        )
        .0;
    }
    // Whatever still comes too near goes: a brace or a kickstand alone, a
    // support with its contact counted lost.
    let left: HashSet<String> = crowding(
        &draft.collections,
        &|_, m| m == model_id,
        input.others,
        NEIGHBOUR_CLEARANCE_MM,
    )
    .into_iter()
    .map(|c| c.id)
    .collect();
    if !left.is_empty() {
        let (gone, _) = take_off(&mut draft.collections, &left);
        lost.extend(gone.iter().map(|c| c.pos));
        placed -= gone.len().min(placed);
    }
    let timings = if timing {
        bread_support_geom::perf::end_frame()
            .map(|f| {
                f.entries
                    .into_iter()
                    .map(|e| (e.label, e.total.as_secs_f64() * 1000.0))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Settled {
        timings,
        state: draft,
        taken: taken.len(),
        placed,
        lost,
        on_neighbours,
        field_ms,
        place_ms,
        brace_ms: clock.elapsed().as_secs_f64() * 1000.0,
    }
}

/// Every entity id of `c`.
fn entity_ids(c: &SupportCollections) -> HashSet<String> {
    c.roots
        .keys()
        .chain(c.trunks.keys())
        .chain(c.branches.keys())
        .chain(c.leaves.keys())
        .chain(c.twigs.keys())
        .chain(c.sticks.keys())
        .chain(c.braces.keys())
        .chain(c.stumps.keys())
        .chain(c.kickstands.keys())
        .chain(c.knots.keys())
        .cloned()
        .collect()
}

/// `c` down to the supports of `model_id`: a knot stays with the shaft it
/// sits on, or with the supports of the model that name it.
fn own_supports(c: &mut SupportCollections, model_id: &str) {
    let on = |m: &str| m == model_id;
    c.roots.retain(|_, e| on(&e.base.model_id));
    c.trunks.retain(|_, e| on(&e.base.model_id));
    c.branches.retain(|_, e| on(&e.base.model_id));
    c.leaves.retain(|_, e| on(&e.base.model_id));
    c.twigs.retain(|_, e| on(&e.base.model_id));
    c.sticks.retain(|_, e| on(&e.base.model_id));
    c.braces.retain(|_, e| on(&e.base.model_id));
    c.stumps.retain(|_, e| on(&e.base.model_id));
    c.kickstands.retain(|_, e| on(&e.base.model_id));
    let mut shafts: HashSet<&str> = HashSet::new();
    for (id, t) in &c.trunks {
        shafts.insert(id);
        shafts.extend(t.segments.iter().map(|s| s.id.as_str()));
    }
    for (id, b) in &c.branches {
        shafts.insert(id);
        shafts.extend(b.segments.iter().map(|s| s.id.as_str()));
    }
    for (id, s) in &c.sticks {
        shafts.insert(id);
        shafts.extend(s.segments.iter().map(|seg| seg.id.as_str()));
    }
    let named = knots_named(c, &|_| true);
    let keep: HashSet<String> = c
        .knots
        .iter()
        .filter(|(id, k)| shafts.contains(k.parent_shaft_id.as_str()) || named.contains(*id))
        .map(|(id, _)| id.clone())
        .collect();
    c.knots.retain(|id, _| keep.contains(id));
}

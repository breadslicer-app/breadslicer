//! What placing shares: the run's context (settings by zone size, the
//! model, ids), the shafts supports hang from, where a tip touches, the
//! checks a member passes (the model, other shafts, room on its host), the
//! branch a member is built as, and the final check of a run.

use std::cell::{Cell, Ref, RefCell};
use std::collections::{HashMap, HashSet};

use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

use crate::auto_support::parameter_sizing::SizingBands;
use crate::auto_support::types::{CandidatePoint, CandidateSource};
use crate::placement_logic::collision_utils::tip_cone_crosses_model;
use crate::placement_logic::pathfinding::sdf_cache::SdfCache;
use crate::placement_logic::placement_utils::calculate_smoothed_normal;
use crate::placement_logic::smart_placement_search_utils::member_departure_angle_from_vertical_deg;
use crate::placement_logic_v3::smart_placement_v3::RouterStats;
use crate::support_types::branch::branch_builder::{
    BranchBuildInput, BranchBuildResult, build_branch_data,
};
use bread_support_geom::bvh::{BvhMesh, Intersection, MeshBvh, Side};
use bread_support_geom::geom::math_utils::{
    js_atan2, js_cos, js_hypot, js_max, js_min, js_sin, js_to_fixed,
};
use bread_support_geom::geom::{Matrix4, Ray};
use bread_support_model::auto_support::constants::{
    ALREADY_SUPPORTED_RADIUS_MM, GRIDLESS_MERGE_RADIUS_MM, MERGE_HOST_LOAD_WEIGHT,
};
use bread_support_model::auto_support::zone_rules::{SupportSize, zone_lives};
use bread_support_model::collision_avoidance::{CollisionField, is_shaft_blocked};
use bread_support_model::constants::member_shaft_diameter_mm;
use bread_support_model::settings::types::SupportSettings;
use bread_support_model::support_kinds::{
    SupportEntityValue, SupportOriginId, SupportTypeId, get_support_type_descriptor,
    near_plate_origin,
};
use bread_support_model::tips::{ContactCone, get_final_socket_position};
use bread_support_model::types::{Branch, Knot, Segment, SupportEntity, SupportState, Trunk, Vec3};

/// The model as placing reads it.
#[derive(Clone, Copy, Debug)]
pub struct PlanMesh<'a> {
    pub bvh: &'a MeshBvh,
    pub matrix: Matrix4,
    /// The faces its rays see.
    pub side: Side,
    /// Its vertex normals, for a contact's smoothed normal.
    pub normals: Option<&'a [[f32; 3]]>,
    /// Its distance field, shared by the run.
    pub sdf: &'a RefCell<SdfCache>,
    /// The faces no tip may land on.
    pub blocked: Option<&'a HashSet<usize>>,
}

impl<'a> PlanMesh<'a> {
    pub fn rays(&self) -> BvhMesh<'_> {
        BvhMesh {
            bvh: self.bvh,
            matrix_world: &self.matrix,
        }
    }

    pub fn field(&self) -> Ref<'a, SdfCache> {
        self.sdf.borrow()
    }

    /// The model's faces a ray from `origin` along `direction` meets,
    /// nearest first.
    pub fn raycast(&self, origin: Vec3, direction: Vec3) -> Vec<Intersection> {
        self.rays().intersect_object(
            &Ray { origin, direction },
            self.side,
            0.0,
            f64::INFINITY,
            false,
        )
    }

    pub fn shaft_blocked(&self, start: Vec3, end: Vec3, radius: f64) -> bool {
        let field = self.field();
        is_shaft_blocked(start, end, radius, &*field)
    }
}

/// What a run shares: the settings (those of the zone size being placed),
/// the size bands, the model, one source of ids and the router's counts.
pub struct PlanCtx<'a> {
    settings: &'a SupportSettings,
    pub bands: &'a SizingBands,
    pub mesh: Option<PlanMesh<'a>>,
    ids: RefCell<&'a mut dyn FnMut() -> String>,
    pub stats: RefCell<RouterStats>,
    /// The settings of each zone size (#360), and the one in use.
    by_zone: Vec<SupportSettings>,
    zone: Cell<Option<usize>>,
    /// A contact picked by hand: its point and its face's normal (#3).
    exact_contact: Cell<Option<(Vec3, Vec3)>>,
    /// The contact being placed lies in a closed cavity (#632).
    enclosed: Cell<bool>,
}

impl<'a> PlanCtx<'a> {
    pub fn new(
        settings: &'a SupportSettings,
        bands: &'a SizingBands,
        mesh: Option<PlanMesh<'a>>,
        new_id: &'a mut dyn FnMut() -> String,
    ) -> Self {
        PlanCtx {
            settings,
            bands,
            mesh,
            ids: RefCell::new(new_id),
            stats: RefCell::new(RouterStats::default()),
            by_zone: zone_lives(settings),
            zone: Cell::new(None),
            exact_contact: Cell::new(None),
            enclosed: Cell::new(false),
        }
    }

    /// The settings in force: the zone size's being placed, when one is.
    pub fn live(&self) -> &SupportSettings {
        self.zone
            .get()
            .and_then(|i| self.by_zone.get(i))
            .unwrap_or(self.settings)
    }

    /// A contact picked by hand, placed exactly there (#3); `None` for a
    /// run's contacts, which look for their surface.
    pub fn set_exact_contact(&self, contact: Option<(Vec3, Vec3)>) {
        self.exact_contact.set(contact);
    }

    /// Where a tip aimed at `tip_pos` touches, and the normal there: the
    /// hand-picked contact when it is that point, else the surface found
    /// round it ([`resolve_surface_normal`], up to 2.25 mm off).
    pub fn contact_at(&self, tip_pos: Vec3) -> (Vec3, Vec3) {
        match self.exact_contact.get() {
            Some((point, normal)) if point == tip_pos => (point, normal),
            _ => resolve_surface_normal(tip_pos, self.mesh.as_ref()),
        }
    }

    /// Whether `cone` goes into the model before its contact (#500): asked
    /// where every tip is built, so that another way is tried at once.
    pub fn cone_crosses(&self, cone: &ContactCone) -> bool {
        self.mesh
            .as_ref()
            .is_some_and(|mesh| contact_cone_crosses_model(cone, mesh))
    }

    /// The size a hand-placed support was asked for, which a bridge keeps
    /// (#564).
    pub fn hand_zone_size(&self) -> Option<&SupportSize> {
        self.exact_contact.get()?;
        self.zone_size()
    }

    /// What follows is placed with zone size `size` (`None`: the run's).
    pub fn set_zone(&self, size: Option<usize>) {
        self.zone.set(size);
    }

    /// Whether what follows lies in a closed cavity (#632).
    pub fn set_enclosed(&self, enclosed: bool) {
        self.enclosed.set(enclosed);
    }

    pub fn is_enclosed(&self) -> bool {
        self.enclosed.get()
    }

    /// The zone size a contact in a closed cavity was given, which its bridge
    /// keeps (#632).
    pub fn enclosed_zone_size(&self) -> Option<&SupportSize> {
        if !self.enclosed.get() {
            return None;
        }
        self.zone_size()
    }

    fn zone_size(&self) -> Option<&SupportSize> {
        self.settings
            .auto_support
            .zone_rules
            .sizes
            .get(self.zone.get()?)
    }

    /// The zone size whose tip is `tip_diameter_mm`.
    pub fn zone_of_tip(&self, tip_diameter_mm: f64) -> Option<usize> {
        self.settings
            .auto_support
            .zone_rules
            .sizes
            .iter()
            .position(|s| (s.tip_diameter_mm - tip_diameter_mm).abs() < 1e-6)
    }

    /// A new id.
    pub fn id(&self) -> String {
        (self.ids.borrow_mut())()
    }
}

impl std::fmt::Debug for PlanCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanCtx").finish_non_exhaustive()
    }
}

/// The most a member leans from vertical, degrees: the branch angle's
/// complement.
pub fn member_max_angle_from_vertical_deg(live: &SupportSettings) -> f64 {
    js_max(0.0, js_min(90.0, 90.0 - live.grid.min_branch_angle_deg))
}

/// How far from vertical a branch leaves its host (0 with no joints).
pub fn branch_departure_angle_deg(branch: &Branch, knot_pos: Vec3) -> f64 {
    branch
        .segments
        .first()
        .and_then(|s| s.top_joint.as_ref())
        .map_or(0.0, |j| {
            member_departure_angle_from_vertical_deg(knot_pos, j.pos)
        })
}

/// `base_id`, or with a number after it when a knot has it already.
pub fn free_knot_id(draft: &SupportState, base_id: &str) -> String {
    let knots = &draft.collections.knots;
    if !knots.contains_key(base_id) {
        return base_id.to_owned();
    }
    let mut n = 2;
    while knots.contains_key(&format!("{base_id}-{n}")) {
        n += 1;
    }
    format!("{base_id}-{n}")
}

/// A trunk, which members hang from.
#[derive(Clone, Copy, Debug)]
pub struct HostEntry<'d> {
    pub host_type_id: SupportTypeId,
    pub host_id: &'d str,
    pub entity: &'d Trunk,
}

/// Every trunk of `draft`, in order.
pub fn collect_host_entities(draft: &SupportState) -> Vec<HostEntry<'_>> {
    draft
        .collections
        .trunks
        .iter()
        .map(|(id, entity)| HostEntry {
            host_type_id: SupportTypeId::Trunk,
            host_id: id,
            entity,
        })
        .collect()
}

/// The shaft a host has: a trunk's, or a stick's (members hang from a stick
/// in a cavity too, #632).
pub fn host_shaft<'d>(
    draft: &'d SupportState,
    host_type_id: SupportTypeId,
    host_id: &str,
) -> Option<(&'d SupportEntity, &'d [Segment])> {
    let c = &draft.collections;
    match host_type_id {
        SupportTypeId::Trunk => c
            .trunks
            .get(host_id)
            .map(|t| (&t.base, t.segments.as_slice())),
        SupportTypeId::Stick => c
            .sticks
            .get(host_id)
            .map(|s| (&s.base, s.segments.as_slice())),
        _ => None,
    }
}

/// The trunk a host is, when it is one.
pub fn host_entity_of<'d>(
    draft: &'d SupportState,
    host_type_id: SupportTypeId,
    host_id: &str,
) -> Option<&'d Trunk> {
    (host_type_id == SupportTypeId::Trunk)
        .then(|| draft.collections.trunks.get(host_id))
        .flatten()
}

/// Where along a shaft a member may hang.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanShaftPoint {
    pub host_type_id: SupportTypeId,
    pub host_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
    pub pos: Vec3,
    pub diameter: f64,
}

/// Points along each segment, ends included.
const SHAFT_SAMPLES_PER_SEGMENT: u32 = 10;
/// The most rounds bare islands are hung in.
pub const MAX_FANNING_PASSES: usize = 5;

/// The points along `segment` (both joints known) of host `id`.
fn points_along(
    out: &mut Vec<FanShaftPoint>,
    kind: SupportTypeId,
    id: &str,
    segment: &Segment,
    start: Vec3,
    end: Vec3,
) {
    for i in 0..=SHAFT_SAMPLES_PER_SEGMENT {
        let t = f64::from(i) / f64::from(SHAFT_SAMPLES_PER_SEGMENT);
        out.push(FanShaftPoint {
            host_type_id: kind,
            host_id: id.to_owned(),
            segment_id: Some(segment.id.clone()),
            t: Some(t),
            pos: Vec3::new(
                start.x + (end.x - start.x) * t,
                start.y + (end.y - start.y) * t,
                start.z + (end.z - start.z) * t,
            ),
            diameter: segment.diameter,
        });
    }
}

/// Whether a segment from `a` to `b` stays further than `reach` from
/// `target` along x or y.
fn out_of_reach(a: Vec3, b: Vec3, target: Vec3, reach: f64) -> bool {
    let apart = |p: f64, q: f64, at: f64| at < p.min(q) - reach || at > p.max(q) + reach;
    apart(a.x, b.x, target.x) || apart(a.y, b.y, target.y)
}

/// The points along every trunk a member may hang from (none of a stump's).
pub fn collect_fan_shaft_points(draft: &SupportState) -> Vec<FanShaftPoint> {
    trunk_points(draft, |_, _| true)
}

/// [`collect_fan_shaft_points`] of the segments within `reach_mm` of
/// `target` on the plate, in the same order.
pub fn collect_fan_shaft_points_near(
    draft: &SupportState,
    target: Vec3,
    reach_mm: f64,
) -> Vec<FanShaftPoint> {
    trunk_points(draft, |a, b| !out_of_reach(a, b, target, reach_mm))
}

fn trunk_points(draft: &SupportState, keep: impl Fn(Vec3, Vec3) -> bool) -> Vec<FanShaftPoint> {
    let near_plate = near_plate_origin();
    let mut points = Vec::new();
    for (id, trunk) in &draft.collections.trunks {
        if trunk.base.origin == Some(near_plate) {
            continue;
        }
        for seg in &trunk.segments {
            let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
                continue;
            };
            if keep(a.pos, b.pos) {
                points_along(&mut points, SupportTypeId::Trunk, id, seg, a.pos, b.pos);
            }
        }
    }
    points
}

/// The points along the sticks within `reach_mm` of `target` on the plate:
/// a contact in a cavity shares a stick from the cavity's floor (#632).
pub fn collect_stick_shaft_points_near(
    draft: &SupportState,
    target: Vec3,
    reach_mm: f64,
) -> Vec<FanShaftPoint> {
    let mut points = Vec::new();
    for (id, stick) in &draft.collections.sticks {
        for seg in &stick.segments {
            let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
                continue;
            };
            if !out_of_reach(a.pos, b.pos, target, reach_mm) {
                points_along(&mut points, SupportTypeId::Stick, id, seg, a.pos, b.pos);
            }
        }
    }
    points
}

/// Where a tip aimed at `tip_pos` touches the model and the normal there:
/// a ray up from 2 mm below first (an underside), then down (a top, normal
/// turned over), each trying a disc of points round it out to 2.25 mm.
///
/// A ray up takes a surface no higher than a 45° slope over its offset (plus
/// 0.5 mm): the tip's own surface (#500), so a point on a face that does not
/// look down finds its face with the ray down, and its support is refused.
pub fn resolve_surface_normal(tip_pos: Vec3, mesh: Option<&PlanMesh<'_>>) -> (Vec3, Vec3) {
    let Some(mesh) = mesh else {
        return (tip_pos, Vec3::new(0.0, 0.0, -1.0));
    };
    const RADII_MM: [f64; 4] = [0.0, 0.75, 1.5, 2.25];
    const SLACK_MM: f64 = 0.5;
    let cast = |up: bool| -> Option<Intersection> {
        let (dz, from_z) = if up {
            (1.0, tip_pos.z - 2.0)
        } else {
            (-1.0, tip_pos.z + 2.0)
        };
        for radius in RADII_MM {
            let steps = if radius == 0.0 { 1 } else { 8 };
            for i in 0..steps {
                let angle = (f64::from(i) / f64::from(steps)) * std::f64::consts::PI * 2.0;
                let origin = Vec3::new(
                    tip_pos.x + js_cos(angle) * radius,
                    tip_pos.y + js_sin(angle) * radius,
                    from_z,
                );
                let highest = tip_pos.z + radius + SLACK_MM;
                let hit = mesh
                    .raycast(origin, Vec3::new(0.0, 0.0, dz))
                    .into_iter()
                    .find(|h| h.point.z >= tip_pos.z - SLACK_MM && (!up || h.point.z <= highest));
                if hit.is_some() {
                    return hit;
                }
            }
        }
        None
    };
    let normal_at = |hit: &Intersection| {
        calculate_smoothed_normal(hit, mesh.bvh.positions(), mesh.normals, &mesh.matrix)
    };
    if let Some(hit) = cast(true) {
        return (hit.point, normal_at(&hit));
    }
    if let Some(hit) = cast(false) {
        let n = normal_at(&hit);
        return (hit.point, Vec3::new(-n.x, -n.y, -n.z));
    }
    (tip_pos, Vec3::new(0.0, 0.0, -1.0))
}

/// Where the supports of `draft` touch the model.
pub fn collect_contact_positions(draft: &SupportState) -> Vec<Vec3> {
    contact_positions_where(draft, &|_| true)
}

/// Where the supports of `draft` of the model `model_id` touch it (#725):
/// a tip on another model of a run holds nothing of this one.
pub fn collect_contact_positions_of(draft: &SupportState, model_id: &str) -> Vec<Vec3> {
    contact_positions_where(draft, &|m| m == model_id)
}

/// Where the supports of `draft` whose model `keep` takes touch it.
fn contact_positions_where(draft: &SupportState, keep: &dyn Fn(&str) -> bool) -> Vec<Vec3> {
    let c = &draft.collections;
    let mut out: Vec<Vec3> = Vec::new();
    out.extend(
        c.trunks
            .values()
            .filter(|t| keep(&t.base.model_id))
            .filter_map(|t| t.contact_cone.as_ref().map(|k| k.pos)),
    );
    out.extend(
        c.branches
            .values()
            .filter(|b| keep(&b.base.model_id))
            .filter_map(|b| b.contact_cone.as_ref().map(|k| k.pos)),
    );
    out.extend(
        c.leaves
            .values()
            .filter(|l| keep(&l.base.model_id))
            .map(|l| l.contact_cone.pos),
    );
    for twig in c.twigs.values().filter(|t| keep(&t.base.model_id)) {
        out.extend([twig.contact_disk_a.pos, twig.contact_disk_b.pos]);
    }
    for stick in c.sticks.values().filter(|s| keep(&s.base.model_id)) {
        out.extend([stick.contact_cone_a.pos, stick.contact_cone_b.pos]);
    }
    out.extend(
        c.stumps
            .values()
            .filter(|s| keep(&s.base.model_id))
            .map(|s| s.contact_cone.pos),
    );
    out
}

/// The candidates no support of `draft` touches already (within 3 mm).
pub fn filter_already_supported(
    candidates: Vec<CandidatePoint>,
    draft: &SupportState,
) -> Vec<CandidatePoint> {
    filter_touched(candidates, &collect_contact_positions(draft))
}

/// [`filter_already_supported`] by the supports of `model_id` alone
/// (#725): in a run of several models, a tip on one leaves the contacts of
/// its neighbour to be held.
pub fn filter_already_supported_on(
    candidates: Vec<CandidatePoint>,
    draft: &SupportState,
    model_id: &str,
) -> Vec<CandidatePoint> {
    filter_touched(candidates, &collect_contact_positions_of(draft, model_id))
}

/// The candidates no point of `touched` lies within 3 mm of.
fn filter_touched(candidates: Vec<CandidatePoint>, touched: &[Vec3]) -> Vec<CandidatePoint> {
    if touched.is_empty() {
        return candidates;
    }
    let reach2 = ALREADY_SUPPORTED_RADIUS_MM * ALREADY_SUPPORTED_RADIUS_MM;
    candidates
        .into_iter()
        .filter(|c| {
            let p = c.tip_pos;
            !touched.iter().any(|t| {
                let (dx, dy, dz) = (p.x - t.x, p.y - t.y, p.z - t.z);
                dx * dx + dy * dy + dz * dz <= reach2
            })
        })
        .collect()
}

/// Whether the way from a knot to a cone's contact meets the model more than
/// 0.5 mm short of it: rays down the middle and 0.25 mm to either side.
pub fn contact_cone_collides(
    knot_pos: Vec3,
    cone_pos: Vec3,
    cone_normal: Vec3,
    surface_normal: Option<Vec3>,
    mesh: &PlanMesh<'_>,
) -> bool {
    let (dx, dy, dz) = (
        cone_pos.x - knot_pos.x,
        cone_pos.y - knot_pos.y,
        cone_pos.z - knot_pos.z,
    );
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    if length < 0.01 {
        return false;
    }
    let way = Vec3::new(dx / length, dy / length, dz / length);
    let n = surface_normal.unwrap_or(cone_normal);
    // Across the way, in the plane of the surface's normal.
    let (sx, sy, sz) = (
        way.y * n.z - way.z * n.y,
        way.z * n.x - way.x * n.z,
        way.x * n.y - way.y * n.x,
    );
    let side = (sx * sx + sy * sy + sz * sz).sqrt();
    let offsets: &[f64] = if side > 0.001 {
        &[0.0, 0.25, -0.25]
    } else {
        &[0.0]
    };
    offsets.iter().any(|&off| {
        let origin = Vec3::new(
            knot_pos.x + (sx / side) * off,
            knot_pos.y + (sy / side) * off,
            knot_pos.z + (sz / side) * off,
        );
        mesh.raycast(origin, way)
            .first()
            .is_some_and(|h| h.distance < length - 0.5)
    })
}

/// Whether `cone` goes into the model before its contact (#500), or comes
/// within its body's radius of another model the field reads (#725).
pub fn contact_cone_crosses_model(cone: &ContactCone, mesh: &PlanMesh<'_>) -> bool {
    tip_cone_crosses_model(cone, mesh.rays()) || cone_meets_obstacles(cone, mesh)
}

/// Whether `cone`, from its socket to its contact, comes within its body's
/// radius of another model of the plate (#725). A field lent out to the
/// router is not asked.
pub fn cone_meets_obstacles(cone: &ContactCone, mesh: &PlanMesh<'_>) -> bool {
    let Ok(field) = mesh.sdf.try_borrow() else {
        return false;
    };
    if field.obstacle_count() == 0 {
        return false;
    }
    let socket = get_final_socket_position(cone);
    let radius = js_max(
        cone.profile.body_diameter_mm,
        cone.profile.contact_diameter_mm,
    ) / 2.0;
    field.obstacles_block(socket, cone.pos, radius)
}

/// The tip cones `entity` touches the model with.
pub fn entity_cones(entity: &SupportEntityValue) -> Vec<&ContactCone> {
    match entity {
        SupportEntityValue::Trunk(e) => e.contact_cone.iter().collect(),
        SupportEntityValue::Branch(e) => e.contact_cone.iter().collect(),
        SupportEntityValue::Leaf(e) => vec![&e.contact_cone],
        SupportEntityValue::Stick(e) => vec![&e.contact_cone_a, &e.contact_cone_b],
        SupportEntityValue::Stump(e) => vec![&e.contact_cone],
        SupportEntityValue::Twig(_)
        | SupportEntityValue::Brace(_)
        | SupportEntityValue::Kickstand(_) => Vec::new(),
    }
}

/// Whether any of `segments` runs into the model's distance field.
pub fn branch_collides_with_sdf(segments: &[Segment], mesh: &PlanMesh<'_>) -> bool {
    segments
        .iter()
        .any(|seg| match (&seg.bottom_joint, &seg.top_joint) {
            (Some(a), Some(b)) => mesh.shaft_blocked(a.pos, b.pos, seg.diameter / 2.0),
            _ => false,
        })
}

/// The ids a knot on a host's shaft names: its segments', or the host's own.
fn shaft_ids<'d>(segments: &'d [Segment], host_id: &'d str) -> HashSet<&'d str> {
    let mut ids: HashSet<&str> = segments.iter().map(|s| s.id.as_str()).collect();
    ids.insert(host_id);
    ids
}

/// How many knots ride a host's shaft (members hang from them; braces ride
/// their own).
pub fn count_attachments_on_host(
    host_type_id: SupportTypeId,
    host_id: &str,
    draft: &SupportState,
) -> usize {
    let Some((_, segments)) = host_shaft(draft, host_type_id, host_id) else {
        return 0;
    };
    let ids = shaft_ids(segments, host_id);
    draft
        .collections
        .knots
        .values()
        .filter(|k| ids.contains(k.parent_shaft_id.as_str()))
        .count()
}

/// Whether a host carries `limit` members already (0: no limit).
pub fn is_host_at_attachment_capacity(
    host_type_id: SupportTypeId,
    host_id: &str,
    limit: f64,
    draft: &SupportState,
) -> bool {
    limit > 0.0 && count_attachments_on_host(host_type_id, host_id, draft) as f64 >= limit
}

/// The longest member a host carries, knot to tip, mm (0 for none).
fn longest_member_mm(host_id: &str, draft: &SupportState) -> f64 {
    let Some((_, segments)) = host_shaft(draft, SupportTypeId::Trunk, host_id) else {
        return 0.0;
    };
    let ids = shaft_ids(segments, host_id);
    let c = &draft.collections;
    let knots: HashMap<&str, Vec3> = c
        .knots
        .values()
        .filter(|k| ids.contains(k.parent_shaft_id.as_str()))
        .map(|k| (k.id.as_str(), k.pos))
        .collect();
    if knots.is_empty() {
        return 0.0;
    }
    let members = c
        .leaves
        .values()
        .map(|l| (&l.parent_knot_id, Some(l.contact_cone.pos)))
        .chain(
            c.branches
                .values()
                .map(|b| (&b.parent_knot_id, b.contact_cone.as_ref().map(|k| k.pos))),
        );
    let mut longest = 0.0;
    for (knot_id, tip) in members {
        let (Some(knot), Some(tip)) = (knots.get(knot_id.as_str()), tip) else {
            continue;
        };
        let span = ((tip.x - knot.x) * (tip.x - knot.x)
            + (tip.y - knot.y) * (tip.y - knot.y)
            + (tip.z - knot.z) * (tip.z - knot.z))
            .sqrt();
        if span > longest {
            longest = span;
        }
    }
    longest
}

/// The origin a support placed for a contact takes from what made it.
pub fn origin_by_source(source: CandidateSource) -> SupportOriginId {
    match source {
        CandidateSource::Voxel
        | CandidateSource::Minima
        | CandidateSource::Intersection
        | CandidateSource::Stabilization => SupportOriginId::Island,
        CandidateSource::Overhang => SupportOriginId::Standalone,
        CandidateSource::Reinforcement => SupportOriginId::Reinforcement,
    }
}

/// The trunk a contact joins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeHost {
    pub host_type_id: SupportTypeId,
    pub host_id: String,
    pub tip_pos: Vec3,
}

/// The trunk of `model_id` a contact at `tip_pos` would join: one whose tip
/// or shaft joint is within 4 mm on the plate, nearest in space (a joint
/// counts 10 % nearer in square), plus half the longest member it carries
/// already. A stump never takes one.
/// With `share`, a trunk of any model of the run takes it (#725).
pub fn find_merge_host(
    tip_pos: Vec3,
    model_id: &str,
    draft: &SupportState,
    share: bool,
) -> Option<MergeHost> {
    let reach2 = GRIDLESS_MERGE_RADIUS_MM * GRIDLESS_MERGE_RADIUS_MM;
    let mut loads: HashMap<String, f64> = HashMap::new();
    let mut best: Option<MergeHost> = None;
    let mut best_score = f64::INFINITY;
    let mut consider = |id: &str, at: Vec3, d2: f64, best: &mut Option<MergeHost>| {
        let load = *loads
            .entry(id.to_owned())
            .or_insert_with(|| longest_member_mm(id, draft));
        let score = js_max(0.0, d2).sqrt() + MERGE_HOST_LOAD_WEIGHT * load;
        if score < best_score {
            best_score = score;
            *best = Some(MergeHost {
                host_type_id: SupportTypeId::Trunk,
                host_id: id.to_owned(),
                tip_pos: at,
            });
        }
    };
    let near_plate = near_plate_origin();
    for (id, trunk) in &draft.collections.trunks {
        if (!share && trunk.base.model_id != model_id) || trunk.base.origin == Some(near_plate) {
            continue;
        }
        if let Some(cone) = &trunk.contact_cone {
            let at = cone.pos;
            let (dx, dy) = (tip_pos.x - at.x, tip_pos.y - at.y);
            if dx * dx + dy * dy <= reach2 {
                let dz = tip_pos.z - at.z;
                consider(id, at, dx * dx + dy * dy + dz * dz, &mut best);
            }
        }
        for seg in &trunk.segments {
            let Some(at) = seg
                .bottom_joint
                .as_ref()
                .or(seg.top_joint.as_ref())
                .map(|j| j.pos)
            else {
                continue;
            };
            let (dx, dy) = (tip_pos.x - at.x, tip_pos.y - at.y);
            if dx * dx + dy * dy > reach2 {
                continue;
            }
            let dz = tip_pos.z - at.z;
            consider(id, at, (dx * dx + dy * dy + dz * dz) * 0.9, &mut best);
        }
    }
    best
}

/// What a hung branch is built from.
#[derive(Debug)]
pub struct HostedBranchInput<'k> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'k str,
    pub parent_knot: &'k Knot,
    pub tip_contact_diameter_mm: Option<f64>,
    pub roots_diameter_mm: Option<f64>,
}

/// A branch hung from a knot, its shaft its share of the host's (#360: no
/// thicker than the host, so its tilted end stays inside it); when that is
/// `rejected` (or its tip cone goes into the model, #500), the band's shaft
/// if thinner.
pub fn build_hosted_branch(
    input: &HostedBranchInput<'_>,
    band_shaft_mm: f64,
    host_diameter_mm: f64,
    ctx: &PlanCtx<'_>,
    rejected: &dyn Fn(&BranchBuildResult) -> bool,
) -> Option<BranchBuildResult> {
    let share = ctx.live().auto_support.member_host_shaft_ratio;
    let band_shaft_mm = band_shaft_mm.min(host_diameter_mm * share);
    let shaft_mm = member_shaft_diameter_mm(band_shaft_mm, host_diameter_mm, share);
    let build = |shaft_mm: f64| {
        let field = ctx.mesh.as_ref().map(|m| m.field());
        build_branch_data(
            &BranchBuildInput {
                tip_pos: input.tip_pos,
                tip_normal: input.tip_normal,
                model_id: input.model_id,
                parent_knot: input.parent_knot,
                mesh: field.as_deref().map(|f| f as &dyn CollisionField),
                shaft_diameter_mm: Some(shaft_mm),
                tip_contact_diameter_mm: input.tip_contact_diameter_mm,
                roots_diameter_mm: input.roots_diameter_mm,
                settings: None,
                tip_profile: None,
            },
            ctx.live(),
            &mut || ctx.id(),
        )
    };
    let refused = |built: &BranchBuildResult| {
        rejected(built)
            || built
                .branch
                .contact_cone
                .as_ref()
                .is_some_and(|cone| ctx.cone_crosses(cone))
    };
    let first = build(shaft_mm);
    if !refused(&first) {
        return Some(first);
    }
    if shaft_mm <= band_shaft_mm {
        return None;
    }
    let thinner = build(band_shaft_mm);
    (!refused(&thinner)).then_some(thinner)
}

fn clamp01(v: f64) -> f64 {
    js_min(1.0, js_max(0.0, v))
}

/// The squared distance between segments `p1`–`p2` and `p3`–`p4`.
pub fn segment_distance_sq(p1: Vec3, p2: Vec3, p3: Vec3, p4: Vec3) -> f64 {
    let (d1x, d1y, d1z) = (p2.x - p1.x, p2.y - p1.y, p2.z - p1.z);
    let (d2x, d2y, d2z) = (p4.x - p3.x, p4.y - p3.y, p4.z - p3.z);
    let (rx, ry, rz) = (p1.x - p3.x, p1.y - p3.y, p1.z - p3.z);
    let a = d1x * d1x + d1y * d1y + d1z * d1z;
    let e = d2x * d2x + d2y * d2y + d2z * d2z;
    let f = d2x * rx + d2y * ry + d2z * rz;
    let (mut s, mut t) = (0.0, 0.0);
    if a < 1e-9 {
        if e >= 1e-9 {
            t = clamp01(f / e);
        }
    } else {
        let c = d1x * rx + d1y * ry + d1z * rz;
        if e < 1e-9 {
            s = clamp01(-c / a);
        } else {
            let b = d1x * d2x + d1y * d2y + d1z * d2z;
            let denom = a * e - b * b;
            if denom != 0.0 {
                s = clamp01((b * f - c * e) / denom);
            }
            t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = clamp01(-c / a);
            } else if t > 1.0 {
                t = 1.0;
                s = clamp01((b - c) / a);
            }
        }
    }
    let dx = (p1.x + d1x * s) - (p3.x + d2x * t);
    let dy = (p1.y + d1y * s) - (p3.y + d2y * t);
    let dz = (p1.z + d1z * s) - (p3.z + d2z * t);
    dx * dx + dy * dy + dz * dz
}

/// Whether a member straight from `knot_pos` to `tip_pos` (radius
/// `leaf_radius_mm`) touches the shaft of a trunk other than `host_id`.
pub fn leaf_path_crosses_supports(
    knot_pos: Vec3,
    tip_pos: Vec3,
    leaf_radius_mm: f64,
    draft: &SupportState,
    host_id: Option<&str>,
) -> bool {
    draft
        .collections
        .trunks
        .iter()
        .filter(|(id, _)| Some(id.as_str()) != host_id)
        .flat_map(|(_, trunk)| &trunk.segments)
        .any(|seg| {
            let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
                return false;
            };
            let clearance = leaf_radius_mm + seg.diameter / 2.0;
            // Further apart than that along one axis is further apart (#445).
            let apart = |p: f64, q: f64, r: f64, s: f64| {
                p.min(q) > r.max(s) + clearance || r.min(s) > p.max(q) + clearance
            };
            !(apart(knot_pos.x, tip_pos.x, a.pos.x, b.pos.x)
                || apart(knot_pos.y, tip_pos.y, a.pos.y, b.pos.y)
                || apart(knot_pos.z, tip_pos.z, a.pos.z, b.pos.z))
                && segment_distance_sq(knot_pos, tip_pos, a.pos, b.pos) < clearance * clearance
        })
}

/// The squared distance from `p` to segment `a`–`b`.
pub fn point_to_segment_distance_sq(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let (abx, aby, abz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    let (apx, apy, apz) = (p.x - a.x, p.y - a.y, p.z - a.z);
    let ab2 = abx * abx + aby * aby + abz * abz;
    if ab2 < 1e-9 {
        return apx * apx + apy * apy + apz * apz;
    }
    let t = js_max(0.0, js_min(1.0, (apx * abx + apy * aby + apz * abz) / ab2));
    let dx = p.x - (a.x + abx * t);
    let dy = p.y - (a.y + aby * t);
    let dz = p.z - (a.z + abz * t);
    dx * dx + dy * dy + dz * dz
}

/// The line of a trunk's segment, which its members' knots lie on: the
/// lowest starts at the top of the base, the highest ends at the tip.
pub fn host_segment_span(
    draft: &SupportState,
    host: &Trunk,
    seg: &Segment,
) -> Option<(Vec3, Vec3)> {
    let start = seg.bottom_joint.as_ref().map(|j| j.pos).or_else(|| {
        draft.collections.roots.get(&host.root_id).map(|r| {
            Vec3::new(
                r.transform.pos.x,
                r.transform.pos.y,
                r.transform.pos.z + r.disk_height + r.cone_height,
            )
        })
    })?;
    let end = seg
        .top_joint
        .as_ref()
        .map(|j| j.pos)
        .or_else(|| host.contact_cone.as_ref().map(|c| c.pos))?;
    Some((start, end))
}

/// The segment `shaft_id` names, of any kind with a shaft, and its owner:
/// none when it names an owner instead (a knot keyed to its host).
fn find_host_segment<'d>(
    draft: &'d SupportState,
    shaft_id: &str,
) -> Option<(SupportTypeId, &'d str, &'d Segment)> {
    let c = &draft.collections;
    macro_rules! look_in {
        ($kind:expr, $owners:expr) => {
            for (owner, entity) in $owners.iter() {
                if let Some(seg) = entity.segments.iter().find(|s| s.id == shaft_id) {
                    return Some(($kind, owner.as_str(), seg));
                }
                if owner == shaft_id {
                    return None;
                }
            }
        };
    }
    look_in!(SupportTypeId::Trunk, c.trunks);
    look_in!(SupportTypeId::Branch, c.branches);
    look_in!(SupportTypeId::Twig, c.twigs);
    look_in!(SupportTypeId::Stick, c.sticks);
    look_in!(SupportTypeId::Stump, c.stumps);
    look_in!(SupportTypeId::Kickstand, c.kickstands);
    None
}

/// A knot keyed to its trunk (not a segment) moved onto the trunk's nearest
/// segment.
pub fn rehost_legacy_knots(draft: &SupportState) -> SupportState {
    let mut out: Option<SupportState> = None;
    for (kid, knot) in &draft.collections.knots {
        let Some(host) = draft.collections.trunks.get(&knot.parent_shaft_id) else {
            continue;
        };
        let mut best: Option<(&Segment, f64)> = None;
        let mut best_d2 = f64::INFINITY;
        for seg in &host.segments {
            let start = seg.bottom_joint.as_ref().map_or(Vec3::ZERO, |j| j.pos);
            let Some(end) = seg.top_joint.as_ref().map(|j| j.pos) else {
                continue;
            };
            let (abx, aby, abz) = (end.x - start.x, end.y - start.y, end.z - start.z);
            let (apx, apy, apz) = (
                knot.pos.x - start.x,
                knot.pos.y - start.y,
                knot.pos.z - start.z,
            );
            let ab2 = abx * abx + aby * aby + abz * abz;
            let t = if ab2 < 1e-9 {
                0.0
            } else {
                js_max(0.0, js_min(1.0, (apx * abx + apy * aby + apz * abz) / ab2))
            };
            let (cx, cy, cz) = (start.x + abx * t, start.y + aby * t, start.z + abz * t);
            let d2 = (knot.pos.x - cx) * (knot.pos.x - cx)
                + (knot.pos.y - cy) * (knot.pos.y - cy)
                + (knot.pos.z - cz) * (knot.pos.z - cz);
            if d2 < best_d2 {
                best_d2 = d2;
                best = Some((seg, t));
            }
        }
        if let Some((seg, t)) = best {
            let k = out
                .get_or_insert_with(|| draft.clone())
                .collections
                .knots
                .get_mut(kid)
                .expect("knot");
            k.parent_shaft_id = seg.id.clone();
            k.t = Some(t);
        }
    }
    out.unwrap_or_else(|| draft.clone())
}

/// Every tip cone's body made the shaft it sits on (shafts grow after cones
/// are built): a trunk's its top segment, a branch's its first, a leaf's
/// `leaf_body_ratio` of its host's segment (#360: narrower, so its tilted base
/// stays inside it).
pub fn sync_contact_cone_diameters_with(
    draft: &SupportState,
    leaf_body_ratio: f64,
) -> SupportState {
    let c = &draft.collections;
    let mut shaft_of: HashMap<&str, f64> = HashMap::new();
    for seg in c
        .trunks
        .values()
        .flat_map(|t| &t.segments)
        .chain(c.branches.values().flat_map(|b| &b.segments))
    {
        if seg.diameter > 0.0 {
            shaft_of.insert(&seg.id, seg.diameter);
        }
    }
    let mut changed = false;
    let mut fit = |cone: &mut ContactCone, body: f64| {
        if (cone.profile.body_diameter_mm - body).abs() > 1e-6 {
            changed = true;
            cone.profile.body_diameter_mm = body;
        }
    };
    let mut next = draft.clone();
    for trunk in next.collections.trunks.values_mut() {
        let top = trunk.segments.last().map(|s| s.diameter);
        if let (Some(cone), Some(d)) = (trunk.contact_cone.as_mut(), top) {
            fit(cone, d);
        }
    }
    for leaf in next.collections.leaves.values_mut() {
        let host = c
            .knots
            .get(&leaf.parent_knot_id)
            .and_then(|k| shaft_of.get(k.parent_shaft_id.as_str()));
        if let Some(&d) = host {
            fit(&mut leaf.contact_cone, d * leaf_body_ratio);
        }
    }
    for branch in next.collections.branches.values_mut() {
        let first = branch.segments.first().map(|s| s.diameter);
        if let (Some(cone), Some(d)) = (branch.contact_cone.as_mut(), first) {
            fit(cone, d);
        }
    }
    if changed { next } else { draft.clone() }
}

/// A support the final check took off (or a member it reports).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanInfo {
    pub id: String,
    pub kind: SupportTypeId,
    pub reason: OrphanReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Placed again with the final sizes (#63, #236).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replaced: bool,
}

impl OrphanInfo {
    /// Whether it was placed again (#63, #236); `None` for one never tried
    /// (its knot or host missing, or kept crossing another shaft).
    pub fn was_placed_again(&self) -> Option<bool> {
        matches!(
            self.reason,
            OrphanReason::HostCulled | OrphanReason::Blocked
        )
        .then_some(self.replaced)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrphanReason {
    MissingKnot,
    MissingHost,
    MissingSegment,
    Drift,
    Cross,
    Blocked,
    HostBlocked,
    /// Its host was taken (#63): it is placed again.
    HostCulled,
}

/// A member (a leaf or branch) as the final check reads it.
struct Member<'d> {
    kind: SupportTypeId,
    id: &'d str,
    model_id: &'d str,
    knot_id: &'d str,
    cone: Option<&'d ContactCone>,
    segments: &'d [Segment],
}

/// The leaves, then the branches.
fn members(draft: &SupportState) -> Vec<Member<'_>> {
    let c = &draft.collections;
    c.leaves
        .iter()
        .map(|(id, l)| Member {
            kind: SupportTypeId::Leaf,
            id,
            model_id: &l.base.model_id,
            knot_id: &l.parent_knot_id,
            cone: Some(&l.contact_cone),
            segments: &[],
        })
        .chain(c.branches.iter().map(|(id, b)| Member {
            kind: SupportTypeId::Branch,
            id,
            model_id: &b.base.model_id,
            knot_id: &b.parent_knot_id,
            cone: b.contact_cone.as_ref(),
            segments: &b.segments,
        }))
        .collect()
}

/// A member's fault: why, its host and knot, and what was found.
type Fault = (OrphanReason, Option<String>, Option<String>, String);

fn at_mm(p: Vec3) -> String {
    format!(
        " @ ({}, {}, Z{})",
        js_to_fixed(p.x, 1),
        js_to_fixed(p.y, 1),
        js_to_fixed(p.z, 1)
    )
}

/// What is wrong with how `member` hangs, if anything: placed again through
/// a model, its knot or host missing, its knot off its host's shaft (0.5 mm),
/// its way blocked by the model or its tip cone into it, leaning more than
/// 75° (or rising under 0.4 mm), or touching another shaft (which keeps it).
fn member_fault(
    member: &Member<'_>,
    draft: &SupportState,
    mesh: Option<&PlanMesh<'_>>,
    crossing: &HashSet<String>,
) -> Option<Fault> {
    let fault = |reason, host: Option<&str>, knot: Option<&str>, detail: String| {
        Some((
            reason,
            host.map(str::to_owned),
            knot.map(str::to_owned),
            detail,
        ))
    };
    if crossing.contains(member.id) {
        return fault(
            OrphanReason::Blocked,
            None,
            Some(member.knot_id),
            "placed again through the mesh".to_owned(),
        );
    }
    if member.knot_id.is_empty() {
        return fault(
            OrphanReason::MissingKnot,
            None,
            None,
            "no parentKnotId".to_owned(),
        );
    }
    let Some(knot) = draft.collections.knots.get(member.knot_id) else {
        return fault(
            OrphanReason::MissingKnot,
            None,
            Some(member.knot_id),
            "knot not in draft".to_owned(),
        );
    };
    let Some((host_kind, owner, seg)) = find_host_segment(draft, &knot.parent_shaft_id) else {
        let reason = if draft.collections.trunks.contains_key(&knot.parent_shaft_id) {
            OrphanReason::MissingSegment
        } else {
            OrphanReason::MissingHost
        };
        return fault(
            reason,
            Some(&knot.parent_shaft_id),
            Some(&knot.id),
            format!("knot {} parent {}", knot.id, knot.parent_shaft_id),
        );
    };
    let trunk = host_entity_of(draft, host_kind, owner);
    // A stick's segment runs joint to joint (members hang there in a cavity,
    // #632).
    let stick_span = (host_kind == SupportTypeId::Stick)
        .then(|| seg.bottom_joint.as_ref().zip(seg.top_joint.as_ref()))
        .flatten()
        .map(|(a, b)| (a.pos, b.pos));
    let Some((start, end)) =
        stick_span.or_else(|| trunk.and_then(|t| host_segment_span(draft, t, seg)))
    else {
        let number = trunk
            .and_then(|t| t.segments.iter().position(|s| s.id == seg.id))
            .map_or(0, |i| i + 1);
        let total = trunk.map_or(0, |t| t.segments.len());
        let origin = trunk
            .and_then(|t| t.base.origin)
            .map_or("unset", SupportOriginId::as_str);
        let short: String = seg.id.chars().take(8).collect();
        let yes = |b: bool| if b { "yes" } else { "no" };
        return fault(
            OrphanReason::MissingHost,
            Some(owner),
            Some(&knot.id),
            format!(
                "segment missing joints (seg {short}, topJoint {}, bottomJoint {}, seg {number}/{total}, origin {origin})",
                yes(seg.top_joint.is_some()),
                yes(seg.bottom_joint.is_some()),
            ),
        );
    };
    let drift2 = point_to_segment_distance_sq(knot.pos, start, end);
    if drift2 > 0.25 {
        return fault(
            OrphanReason::Drift,
            Some(owner),
            Some(&knot.id),
            format!("drift {}mm from shaft", js_to_fixed(drift2.sqrt(), 2)),
        );
    }
    let cone = member.cone?;
    let blocked = mesh.is_some_and(|m| {
        if get_support_type_descriptor(member.kind).has_segments {
            branch_collides_with_sdf(member.segments, m)
        } else {
            contact_cone_collides(knot.pos, cone.pos, cone.normal, cone.surface_normal, m)
        }
    });
    if blocked {
        return fault(
            OrphanReason::Blocked,
            Some(owner),
            Some(&knot.id),
            "knot→tip blocked by mesh".to_owned(),
        );
    }
    if mesh.is_some_and(|m| contact_cone_crosses_model(cone, m)) {
        return fault(
            OrphanReason::Blocked,
            Some(owner),
            Some(&knot.id),
            "tip cone crosses mesh".to_owned(),
        );
    }
    let tip = cone.pos;
    let rise = tip.z - knot.pos.z;
    let across = js_hypot(&[tip.x - knot.pos.x, tip.y - knot.pos.y]);
    let lean = (js_atan2(across, js_max(0.001, rise)) * 180.0) / std::f64::consts::PI;
    const MOST_LEAN_DEG: f64 = 75.0;
    if rise < 0.4 || lean > MOST_LEAN_DEG + 1e-6 {
        let fixed1 = |v: f64| js_to_fixed(v, 1);
        return fault(
            OrphanReason::Blocked,
            Some(owner),
            Some(&knot.id),
            format!(
                "too shallow {}° > {MOST_LEAN_DEG}° (h={} v={})",
                fixed1(lean),
                fixed1(across),
                fixed1(rise)
            ),
        );
    }
    if leaf_path_crosses_supports(knot.pos, tip, 0.25, draft, Some(owner)) {
        return fault(
            OrphanReason::Cross,
            Some(owner),
            Some(&knot.id),
            "leaf/branch crosses another shaft after thickening".to_owned(),
        );
    }
    None
}

/// The final check of a run: takes off every trunk whose shaft goes through
/// the model (short of its last tenth) or whose tip cone does, every stick
/// or stump whose tip cone does, every member that hangs wrong
/// ([`member_fault`]) or whose host was taken, and the knots and bases left
/// with nothing; what it took, reported. `crossing` names supports placed
/// again and found through a model (#236).
pub fn validate_and_cull_orphans(
    draft: &SupportState,
    mesh: Option<&PlanMesh<'_>>,
    crossing: &HashSet<String>,
) -> (SupportState, Vec<OrphanInfo>) {
    validate_and_cull_orphans_of(draft, mesh, crossing, None)
}

/// [`validate_and_cull_orphans`], checking only the supports of
/// `model_id` when given (#725): in a run of several models the others'
/// were checked on their own model, and `mesh`'s field holds their model as
/// a neighbour it keeps clear of. A member of another model whose host is
/// taken still goes.
pub fn validate_and_cull_orphans_of(
    draft: &SupportState,
    mesh: Option<&PlanMesh<'_>>,
    crossing: &HashSet<String>,
    model_id: Option<&str>,
) -> (SupportState, Vec<OrphanInfo>) {
    let ours = |m: &str| model_id.is_none_or(|id| id == m);
    let mut taken: Vec<OrphanInfo> = Vec::new();
    let report =
        |id: &str, kind, reason, host: Option<String>, knot: Option<String>, detail| OrphanInfo {
            id: id.to_owned(),
            kind,
            reason,
            host_id: host,
            knot_id: knot,
            detail: Some(detail),
            replaced: false,
        };
    // What was taken that members hang from, and its kind's name.
    let mut taken_hosts: HashMap<String, &'static str> = HashMap::new();
    let mut trunks_out: IndexSet<String> = IndexSet::new();
    let mut sticks_out: HashSet<String> = HashSet::new();
    let mut stumps_out: HashSet<String> = HashSet::new();

    if let Some(mesh) = mesh {
        for (id, trunk) in &draft.collections.trunks {
            if !ours(&trunk.base.model_id) {
                continue;
            }
            let through = crossing.contains(id)
                || trunk.segments.iter().any(|seg| {
                    let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
                        return false;
                    };
                    let tip = trunk.contact_cone.as_ref().map_or(b.pos, |c| c.pos);
                    let short = Vec3::new(
                        b.pos.x * 0.9 + tip.x * 0.1,
                        b.pos.y * 0.9 + tip.y * 0.1,
                        b.pos.z * 0.9 + tip.z * 0.1,
                    );
                    mesh.shaft_blocked(a.pos, short, seg.diameter / 2.0 + 0.15)
                });
            let cone_in = !through
                && trunk
                    .contact_cone
                    .as_ref()
                    .is_some_and(|c| contact_cone_crosses_model(c, mesh));
            if through || cone_in {
                let name = get_support_type_descriptor(SupportTypeId::Trunk).singular;
                let at = trunk
                    .contact_cone
                    .as_ref()
                    .map_or(String::new(), |c| at_mm(c.pos));
                let what = if cone_in {
                    "tip cone crosses mesh"
                } else {
                    "shaft pierces mesh"
                };
                taken_hosts.insert(id.clone(), name);
                taken.push(report(
                    id,
                    SupportTypeId::Trunk,
                    OrphanReason::HostBlocked,
                    None,
                    None,
                    format!("{name} {what}{at}"),
                ));
                trunks_out.insert(id.clone());
            }
        }
        // A stick in a cavity may carry members (#632): they go with it.
        for (id, stick) in &draft.collections.sticks {
            if !ours(&stick.base.model_id) {
                continue;
            }
            if let Some(cone) = [&stick.contact_cone_a, &stick.contact_cone_b]
                .into_iter()
                .find(|c| contact_cone_crosses_model(c, mesh))
            {
                taken.push(report(
                    id,
                    SupportTypeId::Stick,
                    OrphanReason::Blocked,
                    None,
                    None,
                    format!("Stick tip cone crosses mesh{}", at_mm(cone.pos)),
                ));
                sticks_out.insert(id.clone());
                taken_hosts.insert(id.clone(), "Stick");
            }
        }
        for (id, stump) in &draft.collections.stumps {
            if !ours(&stump.base.model_id) {
                continue;
            }
            if contact_cone_crosses_model(&stump.contact_cone, mesh) {
                taken.push(report(
                    id,
                    SupportTypeId::Stump,
                    OrphanReason::Blocked,
                    None,
                    None,
                    format!(
                        "Stump tip cone crosses mesh{}",
                        at_mm(stump.contact_cone.pos)
                    ),
                ));
                stumps_out.insert(id.clone());
            }
        }
    }

    let members = members(draft);
    let mut members_out: HashSet<(SupportTypeId, &str)> = HashSet::new();
    for member in members.iter().filter(|m| ours(m.model_id)) {
        if let Some((reason, host, knot, detail)) = member_fault(member, draft, mesh, crossing) {
            taken.push(report(member.id, member.kind, reason, host, knot, detail));
            if reason != OrphanReason::Cross {
                members_out.insert((member.kind, member.id));
            }
        }
    }
    for member in &members {
        if members_out.contains(&(member.kind, member.id)) {
            continue;
        }
        let Some(knot) = draft.collections.knots.get(member.knot_id) else {
            continue;
        };
        let host = find_host_segment(draft, &knot.parent_shaft_id)
            .map_or(knot.parent_shaft_id.as_str(), |(_, owner, _)| owner);
        if let Some(name) = taken_hosts.get(host) {
            taken.push(report(
                member.id,
                member.kind,
                OrphanReason::HostCulled,
                Some(host.to_owned()),
                Some(knot.id.clone()),
                format!("{name} culled (blocked)"),
            ));
            members_out.insert((member.kind, member.id));
        }
    }

    if members_out.is_empty()
        && trunks_out.is_empty()
        && sticks_out.is_empty()
        && stumps_out.is_empty()
    {
        return (draft.clone(), taken);
    }

    // The knots something left still hangs from (braces' are never taken).
    let mut still_used: HashSet<&str> = members
        .iter()
        .filter(|m| !members_out.contains(&(m.kind, m.id)) && !m.knot_id.is_empty())
        .map(|m| m.knot_id)
        .collect();
    for brace in draft.collections.braces.values() {
        still_used.insert(&brace.start_knot_id);
        still_used.insert(&brace.end_knot_id);
    }
    let mut knots_out: HashSet<&str> = members
        .iter()
        .filter(|m| members_out.contains(&(m.kind, m.id)))
        .map(|m| m.knot_id)
        .filter(|k| !k.is_empty() && !still_used.contains(k))
        .collect();
    let c = &draft.collections;
    let mut segments_out: HashSet<&str> = HashSet::new();
    for id in &trunks_out {
        segments_out.extend(
            c.trunks
                .get(id)
                .into_iter()
                .flat_map(|t| &t.segments)
                .map(|s| s.id.as_str()),
        );
    }
    for id in &sticks_out {
        segments_out.extend(
            c.sticks
                .get(id)
                .into_iter()
                .flat_map(|s| &s.segments)
                .map(|s| s.id.as_str()),
        );
    }
    for (kid, knot) in &c.knots {
        let on = knot.parent_shaft_id.as_str();
        if (segments_out.contains(on) || trunks_out.contains(on) || sticks_out.contains(on))
            && !still_used.contains(kid.as_str())
        {
            knots_out.insert(kid);
        }
    }

    let bases_out: Vec<String> = trunks_out
        .iter()
        .filter_map(|id| c.trunks.get(id).map(|t| t.root_id.clone()))
        .collect();
    let mut next = draft.clone();
    let n = &mut next.collections;
    n.leaves
        .retain(|id, _| !members_out.contains(&(SupportTypeId::Leaf, id.as_str())));
    n.branches
        .retain(|id, _| !members_out.contains(&(SupportTypeId::Branch, id.as_str())));
    n.knots.retain(|id, _| !knots_out.contains(id.as_str()));
    n.sticks.retain(|id, _| !sticks_out.contains(id));
    n.stumps.retain(|id, _| !stumps_out.contains(id));
    n.trunks.retain(|id, _| !trunks_out.contains(id));
    for base in bases_out {
        // A base goes when no trunk left stands on it.
        if !base.is_empty()
            && n.roots.contains_key(&base)
            && !n.trunks.values().any(|t| t.root_id == base)
        {
            n.roots.shift_remove(&base);
        }
    }
    (next, taken)
}

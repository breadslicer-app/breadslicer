//! DragonFruit `src/supports/autoSupport/autoPlace.ts`, the helpers: the run's
//! context, host resolution, surface normals, the already-supported filter,
//! leaf/branch collision checks, attachment capacity, the merge-host search,
//! the hosted-branch builder, the fan shaft pool, segment spans, legacy knot
//! rehosting, cone/shaft diameter sync and the orphan cull.

use std::cell::{Cell, Ref, RefCell};
use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};

use crate::supports::auto_support::constants::{GRIDLESS_MERGE_RADIUS_MM, MERGE_HOST_LOAD_WEIGHT};
use crate::supports::auto_support::parameter_sizing::SizingBands;
use crate::supports::auto_support::types::CandidateSource;
use crate::supports::constants::member_shaft_diameter_mm;
use crate::supports::placement_logic::collision_avoidance::{CollisionField, is_shaft_blocked};
use crate::supports::placement_logic::collision_utils::tip_cone_crosses_model;
use crate::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use crate::supports::placement_logic::placement_utils::calculate_smoothed_normal;
use crate::supports::placement_logic::smart_placement_search_utils::member_departure_angle_from_vertical_deg;
use crate::supports::placement_logic_v3::smart_placement_v3::RouterStats;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::types::ContactCone;
use crate::supports::support_type_registry::{
    ConeBodyFollows, SHAFT_HOSTED_MEMBER_TYPE_ORDER, SupportEntityValue, SupportOriginId,
    SupportTypeId, get_support_type_descriptor, near_plate_origin,
};
use crate::supports::support_types::branch::branch_builder::{
    BranchBuildInput, BranchBuildResult, build_branch_data,
};
use crate::supports::types::{Branch, Knot, Segment, SupportState, Trunk, Vec3};
use crate::three::math_utils::{js_atan2, js_cos, js_hypot, js_max, js_min, js_sin};
use crate::three::{Matrix4, Ray};
use crate::three_mesh_bvh::{BvhMesh, Intersection, MeshBvh, Side};

/// The model mesh as the plan reads it: DragonFruit's `THREE.Mesh`.
#[derive(Clone, Copy)]
pub struct PlanMesh<'a> {
    pub bvh: &'a MeshBvh,
    pub matrix: Matrix4,
    /// The material side its raycasts honour.
    pub side: Side,
    /// The geometry's vertex normals, for the smoothed contact normal.
    pub normals: Option<&'a [[f32; 3]]>,
    /// The mesh's pooled distance field.
    pub sdf: &'a RefCell<SdfCache>,
    /// The model's support-blocker mask.
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

    /// `raycaster.intersectObject(mesh, false)` from `origin` along `direction`.
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

/// What the whole plan shares: the live settings (`getSettings()`), the band
/// table, the mesh, the run's one id source and the router counters.
pub struct PlanCtx<'a> {
    base_live: &'a SupportSettings,
    pub bands: &'a SizingBands,
    pub mesh: Option<PlanMesh<'a>>,
    ids: RefCell<&'a mut dyn FnMut() -> String>,
    pub stats: RefCell<RouterStats>,
    /// SoulCrafted (#360): the live settings of each zone size, and the one
    /// the support being placed takes.
    zone_lives: Vec<SupportSettings>,
    active_zone: Cell<Option<usize>>,
    /// SoulCrafted (#3): a contact placed by hand, its point and its face's
    /// normal. Its tip goes there and nowhere else ([`Self::contact_at`]).
    exact_contact: Cell<Option<(Vec3, Vec3)>>,
    /// SoulCrafted (#445): a fan reads only the shafts in its reach.
    near_fans: Cell<bool>,
}

impl<'a> PlanCtx<'a> {
    pub fn new(
        live: &'a SupportSettings,
        bands: &'a SizingBands,
        mesh: Option<PlanMesh<'a>>,
        new_id: &'a mut dyn FnMut() -> String,
    ) -> Self {
        PlanCtx {
            base_live: live,
            bands,
            mesh,
            ids: RefCell::new(new_id),
            stats: RefCell::new(RouterStats::default()),
            zone_lives: zone_lives(live),
            active_zone: Cell::new(None),
            exact_contact: Cell::new(None),
            near_fans: Cell::new(false),
        }
    }

    /// The live settings (`getSettings()`): those of the zone size the
    /// support being placed takes, when one is set.
    pub fn live(&self) -> &SupportSettings {
        self.active_zone
            .get()
            .and_then(|i| self.zone_lives.get(i))
            .unwrap_or(self.base_live)
    }

    /// Places the contact at `point`, with `normal`, exactly there (#3): a
    /// point picked by hand on the surface the person sees. `None` lets
    /// every contact find its surface as a run's does.
    pub fn set_exact_contact(&self, contact: Option<(Vec3, Vec3)>) {
        self.exact_contact.set(contact);
    }

    /// With `near`, a fan reads only the shafts in its reach
    /// ([`collect_fan_shaft_points_near`]): it takes the same host, and its
    /// refusal no longer tells how far the nearest one out of reach stands.
    pub fn set_near_fans(&self, near: bool) {
        self.near_fans.set(near);
    }

    /// Whether a fan reads only the shafts in its reach.
    pub fn near_fans(&self) -> bool {
        self.near_fans.get()
    }

    /// The surface point and normal a tip aimed at `tip_pos` takes: the
    /// exact contact when it is that point, else what
    /// [`resolve_surface_normal`] finds around it. That search walks a disc
    /// of rays up to 2.25 mm off the point and takes a surface there: right
    /// for a run's candidates, wrong for a point the person picked.
    pub fn contact_at(&self, tip_pos: Vec3) -> (Vec3, Vec3) {
        match self.exact_contact.get() {
            Some((point, normal)) if point == tip_pos => (point, normal),
            _ => resolve_surface_normal(
                tip_pos,
                self.mesh.as_ref(),
                self.live().auto_support.tips_on_own_surface,
            ),
        }
    }

    /// SoulCrafted (#500): whether `cone` may not be placed, going into the
    /// model before its contact, with
    /// [`AutoSupportSettings::tip_cones_clear`]. Asked of every tip where it
    /// is built, so that what would place it tries its next way to the
    /// contact (another host, another route, a trunk of its own) instead of
    /// leaving the cull at the end of the plan to take it away.
    ///
    /// [`AutoSupportSettings::tip_cones_clear`]: crate::supports::auto_support::settings::AutoSupportSettings::tip_cones_clear
    pub fn cone_crosses(&self, cone: &ContactCone) -> bool {
        self.live().auto_support.tip_cones_clear
            && self
                .mesh
                .as_ref()
                .is_some_and(|mesh| contact_cone_crosses_model(cone, mesh))
    }

    /// The zone size a support placed by hand was asked for (a contact
    /// picked exactly, with a size set): the person's own numbers, which a
    /// bridge keeps instead of a run's thinnest size (#564).
    pub fn hand_zone_size(
        &self,
    ) -> Option<&crate::supports::auto_support::zone_rules::SupportSize> {
        self.exact_contact.get()?;
        let i = self.active_zone.get()?;
        self.base_live
            .auto_support
            .zone_rules
            .as_ref()?
            .sizes
            .get(i)
    }

    /// Places what follows with zone size `size` (`None`: the run's own).
    pub fn set_zone(&self, size: Option<usize>) {
        self.active_zone.set(size);
    }

    /// The zone size whose tip a contact of `tip_diameter_mm` was drawn
    /// with, when the run has zone rules.
    pub fn zone_of_tip(&self, tip_diameter_mm: f64) -> Option<usize> {
        let rules = self.base_live.auto_support.zone_rules.as_ref()?;
        rules
            .sizes
            .iter()
            .position(|s| (s.tip_diameter_mm - tip_diameter_mm).abs() < 1e-6)
    }

    /// `uuidv4()`.
    pub fn id(&self) -> String {
        (self.ids.borrow_mut())()
    }
}

/// The live settings of every zone size of `live`'s zone rules: `live` with
/// the size's tip, trunk and base, and the size as its sizing preset (the
/// band table knows it by that id). Empty without zone rules.
pub fn zone_lives(live: &SupportSettings) -> Vec<SupportSettings> {
    let Some(rules) = live.auto_support.zone_rules.as_ref() else {
        return Vec::new();
    };
    (0..rules.sizes.len())
        .map(|i| {
            let mut sized = live.clone();
            apply_zone_size(&mut sized, rules, i);
            sized
        })
        .collect()
}

/// Writes zone size `index` of `rules` into `settings`' tip, shaft and roots.
pub fn apply_zone_size(
    settings: &mut SupportSettings,
    rules: &crate::supports::auto_support::zone_rules::ZoneRules,
    index: usize,
) {
    let size = &rules.sizes[index];
    settings.tip.contact_diameter_mm = size.tip_diameter_mm;
    settings.tip.body_diameter_mm = size.trunk_diameter_mm;
    settings.tip.length_mm = size.tip_length_mm;
    settings.tip.penetration_mm = size.penetration_mm;
    settings.tip.breakpoint_mm = Some(size.break_point_mm);
    settings.shaft.diameter_mm = size.trunk_diameter_mm;
    settings.shaft.secondary_diameter_mm = Some(size.trunk_diameter_mm);
    settings.roots.diameter_mm = size.base_diameter_mm;
    settings.roots.disk_height_mm = size.base_thickness_mm;
    // The base is a disk; its join cone is drawn on its own (on the raft
    // when there is one).
    settings.roots.cone_height_mm = 0.0;
    settings.roots.join_cone_mm = Some(size.join_cone_mm);
    settings.roots.neck_diameter_mm = size.trunk_diameter_mm;
    settings.mesh_to_mesh.base_tip = size.base_tip;
    settings.auto_support.sizing_preset = rules.band_id(index);
}

/// The steepest angle from vertical an auto member or chunk link may lean,
/// from the user's branch-angle rule.
pub fn member_max_angle_from_vertical_deg(live: &SupportSettings) -> f64 {
    let min_rise_deg = live.grid.min_branch_angle_deg;
    js_max(0.0, js_min(90.0, 90.0 - min_rise_deg))
}

/// The angle a branch's shaft leaves its host at. 0 when it has no joints yet.
pub fn branch_departure_angle_deg(branch: &Branch, knot_pos: Vec3) -> f64 {
    branch
        .segments
        .first()
        .and_then(|s| s.top_joint.as_ref())
        .map_or(0.0, |j| {
            member_departure_angle_from_vertical_deg(knot_pos, j.pos)
        })
}

/// A knot id free in the draft: auto knot ids are built from candidate ids,
/// which repeat across scans.
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

/// Exact volume (mm³) of a closed mesh via the signed tetrahedron sum.
pub fn compute_mesh_volume_mm3(bvh: &MeshBvh) -> f64 {
    let pos = bvh.positions();
    let at = |i: u32| {
        let p = pos[i as usize];
        (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
    };
    let mut vol = 0.0;
    for tri in bvh.index().chunks(3) {
        let (ax, ay, az) = at(tri[0]);
        let (bx, by, bz) = at(tri[1]);
        let (cx, cy, cz) = at(tri[2]);
        vol +=
            (ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx)) / 6.0;
    }
    vol.abs()
}

/// One host entity in the draft, with the type that owns it.
#[derive(Clone, Copy)]
pub struct HostEntry<'d> {
    pub host_type_id: SupportTypeId,
    pub host_id: &'d str,
    pub entity: &'d Trunk,
}

/// Every host entity in the draft, in registry order (the trunks: the one
/// `canBeGridHost` type).
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

/// The host entity `(host_type_id, host_id)` names.
pub fn host_entity_of<'d>(
    draft: &'d SupportState,
    host_type_id: SupportTypeId,
    host_id: &str,
) -> Option<&'d Trunk> {
    (host_type_id == SupportTypeId::Trunk)
        .then(|| draft.collections.trunks.get(host_id))
        .flatten()
}

/// Resolve the real surface normal at a candidate's tip by raycasting: an
/// upward ray from just below the tip first (underside contact), then a
/// downward one (top-surface contact, normal flipped), each walking a small
/// disc of offsets; the placeholder when both miss or there is no mesh.
///
/// SoulCrafted (#500), with `own_surface`: an upward ray takes a surface no
/// higher above the tip than a 45 degree slope rises over the ray's offset,
/// plus the tolerance: the tip's own surface. Without it, as DragonFruit
/// does, it takes the first surface at any height, so a candidate on a
/// surface that does not face down (an up-facing point of an overhang
/// region, a minimum inside the model) goes to whatever hangs above it, 30 mm
/// up on a mini's curled fingers, and piles tips there. With it, such a
/// candidate finds its own surface with the downward ray, and its support,
/// which would come up through the model, is refused.
pub fn resolve_surface_normal(
    tip_pos: Vec3,
    mesh: Option<&PlanMesh<'_>>,
    own_surface: bool,
) -> (Vec3, Vec3) {
    let Some(mesh) = mesh else {
        return (tip_pos, Vec3::new(0.0, 0.0, -1.0));
    };
    const SEARCH_RADII_MM: [f64; 4] = [0.0, 0.75, 1.5, 2.25];
    const SAME_SIDE_TOLERANCE_MM: f64 = 0.5;

    let cast_from_disc = |direction_z: f64| -> Option<Intersection> {
        let seed_z = if direction_z == 1.0 {
            tip_pos.z - 2.0
        } else {
            tip_pos.z + 2.0
        };
        for radius_mm in SEARCH_RADII_MM {
            let steps = if radius_mm == 0.0 { 1 } else { 8 };
            for i in 0..steps {
                let angle = (f64::from(i) / f64::from(steps)) * std::f64::consts::PI * 2.0;
                let origin = Vec3::new(
                    tip_pos.x + js_cos(angle) * radius_mm,
                    tip_pos.y + js_sin(angle) * radius_mm,
                    seed_z,
                );
                let highest = if own_surface {
                    tip_pos.z + radius_mm + SAME_SIDE_TOLERANCE_MM
                } else {
                    f64::INFINITY
                };
                if let Some(hit) = mesh
                    .raycast(origin, Vec3::new(0.0, 0.0, direction_z))
                    .into_iter()
                    .find(|h| {
                        h.point.z >= tip_pos.z - SAME_SIDE_TOLERANCE_MM
                            && (direction_z < 0.0 || h.point.z <= highest)
                    })
                {
                    return Some(hit);
                }
            }
        }
        None
    };
    let smoothed = |hit: &Intersection| {
        calculate_smoothed_normal(hit, mesh.bvh.positions(), mesh.normals, &mesh.matrix)
    };

    if let Some(up) = cast_from_disc(1.0) {
        return (up.point, smoothed(&up));
    }
    if let Some(down) = cast_from_disc(-1.0) {
        let n = smoothed(&down);
        return (down.point, Vec3::new(-n.x, -n.y, -n.z));
    }
    (tip_pos, Vec3::new(0.0, 0.0, -1.0))
}

/// Where every support currently touches the model: every type's declared
/// contacts (`contactEndpointsFor`, lower end first), in registry order.
pub fn collect_contact_positions(snapshot: &SupportState) -> Vec<Vec3> {
    use crate::supports::support_type_registry::contact_endpoints_for;
    let c = &snapshot.collections;
    let mut out = Vec::new();
    for field in contact_endpoints_for(SupportTypeId::Trunk)
        .iter()
        .map(|e| e.field)
    {
        debug_assert_eq!(field, "contactCone");
        out.extend(
            c.trunks
                .values()
                .filter_map(|e| e.contact_cone.as_ref().map(|k| k.pos)),
        );
    }
    for _ in contact_endpoints_for(SupportTypeId::Branch) {
        out.extend(
            c.branches
                .values()
                .filter_map(|e| e.contact_cone.as_ref().map(|k| k.pos)),
        );
    }
    for _ in contact_endpoints_for(SupportTypeId::Leaf) {
        out.extend(c.leaves.values().map(|e| e.contact_cone.pos));
    }
    let twig_fields: Vec<&str> = contact_endpoints_for(SupportTypeId::Twig)
        .iter()
        .map(|e| e.field)
        .collect();
    for e in c.twigs.values() {
        for f in &twig_fields {
            out.push(if *f == "contactDiskA" {
                e.contact_disk_a.pos
            } else {
                e.contact_disk_b.pos
            });
        }
    }
    let stick_fields: Vec<&str> = contact_endpoints_for(SupportTypeId::Stick)
        .iter()
        .map(|e| e.field)
        .collect();
    for e in c.sticks.values() {
        for f in &stick_fields {
            out.push(if *f == "contactConeA" {
                e.contact_cone_a.pos
            } else {
                e.contact_cone_b.pos
            });
        }
    }
    for _ in contact_endpoints_for(SupportTypeId::Stump) {
        out.extend(c.stumps.values().map(|e| e.contact_cone.pos));
    }
    out
}

/// Remove candidates whose tip already carries a support contact.
pub fn filter_already_supported(
    candidates: Vec<crate::supports::auto_support::types::CandidatePoint>,
    draft: &SupportState,
) -> Vec<crate::supports::auto_support::types::CandidatePoint> {
    let existing = collect_contact_positions(draft);
    if existing.is_empty() {
        return candidates;
    }
    let r2 = crate::supports::auto_support::constants::ALREADY_SUPPORTED_RADIUS_MM
        * crate::supports::auto_support::constants::ALREADY_SUPPORTED_RADIUS_MM;
    candidates
        .into_iter()
        .filter(|c| {
            let p = &c.tip_pos;
            !existing.iter().any(|tip| {
                let dx = p.x - tip.x;
                let dy = p.y - tip.y;
                let dz = p.z - tip.z;
                dx * dx + dy * dy + dz * dz <= r2
            })
        })
        .collect()
}

/// Whether a leaf cone from `knot_pos` to the cone's contact hits the model
/// before the contact: three rays (centre and ±0.25 mm across) from the knot.
pub fn contact_cone_collides(
    knot_pos: Vec3,
    cone_pos: Vec3,
    cone_normal: Vec3,
    surface_normal: Option<Vec3>,
    mesh: &PlanMesh<'_>,
) -> bool {
    let dx = cone_pos.x - knot_pos.x;
    let dy = cone_pos.y - knot_pos.y;
    let dz = cone_pos.z - knot_pos.z;
    let total = (dx * dx + dy * dy + dz * dz).sqrt();
    if total < 0.01 {
        return false;
    }
    let dir = Vec3::new(dx / total, dy / total, dz / total);
    let n = surface_normal.unwrap_or(cone_normal);
    let px = dir.y * n.z - dir.z * n.y;
    let py = dir.z * n.x - dir.x * n.z;
    let pz = dir.x * n.y - dir.y * n.x;
    let plen = (px * px + py * py + pz * pz).sqrt();
    let offsets: &[f64] = if plen > 0.001 {
        &[0.0, 0.25, -0.25]
    } else {
        &[0.0]
    };
    for &off in offsets {
        let origin = Vec3::new(
            knot_pos.x + (px / plen) * off,
            knot_pos.y + (py / plen) * off,
            knot_pos.z + (pz / plen) * off,
        );
        let hits = mesh.raycast(origin, dir);
        if hits.first().is_some_and(|h| h.distance < total - 0.5) {
            return true;
        }
    }
    false
}

/// SoulCrafted (#500): whether `cone` goes into the model before its contact
/// ([`tip_cone_crosses_model`]).
pub fn contact_cone_crosses_model(cone: &ContactCone, mesh: &PlanMesh<'_>) -> bool {
    tip_cone_crosses_model(cone, mesh.rays())
}

/// The tip cones `entity` reaches the model with.
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

/// Whether any segment of a built branch is blocked by the distance field.
pub fn branch_collides_with_sdf(segments: &[Segment], mesh: &PlanMesh<'_>) -> bool {
    segments
        .iter()
        .any(|seg| match (&seg.bottom_joint, &seg.top_joint) {
            (Some(a), Some(b)) => mesh.shaft_blocked(a.pos, b.pos, seg.diameter / 2.0),
            _ => false,
        })
}

/// Knots (branches + leaves) riding the host's shaft, legacy entity-id knots
/// included. Brace knots ride their own span prefix and are not counted.
pub fn count_attachments_on_host(
    host_type_id: SupportTypeId,
    host_id: &str,
    draft: &SupportState,
) -> usize {
    let Some(host) = host_entity_of(draft, host_type_id, host_id) else {
        return 0;
    };
    let mut segment_ids: HashSet<&str> = host.segments.iter().map(|s| s.id.as_str()).collect();
    segment_ids.insert(host_id);
    draft
        .collections
        .knots
        .values()
        .filter(|k| segment_ids.contains(k.parent_shaft_id.as_str()))
        .count()
}

/// Longest hosted member span (mm) on a host, knot to tip over its leaves and
/// branches; zero when it carries nothing.
pub fn max_member_span_mm(host_type_id: SupportTypeId, host_id: &str, draft: &SupportState) -> f64 {
    let Some(host) = host_entity_of(draft, host_type_id, host_id) else {
        return 0.0;
    };
    let mut segment_ids: HashSet<&str> = host.segments.iter().map(|s| s.id.as_str()).collect();
    segment_ids.insert(host_id);
    let knot_by_id: HashMap<&str, Vec3> = draft
        .collections
        .knots
        .values()
        .filter(|k| segment_ids.contains(k.parent_shaft_id.as_str()))
        .map(|k| (k.id.as_str(), k.pos))
        .collect();
    if knot_by_id.is_empty() {
        return 0.0;
    }
    let mut longest = 0.0;
    for member_type in SHAFT_HOSTED_MEMBER_TYPE_ORDER {
        for (knot_id, tip) in members_of(draft, member_type) {
            let (Some(knot_pos), Some(tip)) = (knot_by_id.get(knot_id.as_str()), tip) else {
                continue;
            };
            let span = ((tip.x - knot_pos.x) * (tip.x - knot_pos.x)
                + (tip.y - knot_pos.y) * (tip.y - knot_pos.y)
                + (tip.z - knot_pos.z) * (tip.z - knot_pos.z))
                .sqrt();
            if span > longest {
                longest = span;
            }
        }
    }
    longest
}

/// `(parentKnotId, contactCone?.pos)` of every member of a shaft-hosted type.
fn members_of(draft: &SupportState, type_id: SupportTypeId) -> Vec<(String, Option<Vec3>)> {
    let c = &draft.collections;
    match type_id {
        SupportTypeId::Leaf => c
            .leaves
            .values()
            .map(|l| (l.parent_knot_id.clone(), Some(l.contact_cone.pos)))
            .collect(),
        SupportTypeId::Branch => c
            .branches
            .values()
            .map(|b| {
                (
                    b.parent_knot_id.clone(),
                    b.contact_cone.as_ref().map(|k| k.pos),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether the host has reached its attachment capacity.
pub fn is_host_at_attachment_capacity(
    host_type_id: SupportTypeId,
    host_id: &str,
    limit: f64,
    draft: &SupportState,
) -> bool {
    if limit <= 0.0 {
        return false;
    }
    count_attachments_on_host(host_type_id, host_id, draft) as f64 >= limit
}

/// The origin each candidate source stamps on the support it builds.
pub fn origin_by_source(source: CandidateSource) -> SupportOriginId {
    match source {
        CandidateSource::Voxel | CandidateSource::Minima | CandidateSource::Intersection => {
            SupportOriginId::Island
        }
        CandidateSource::Overhang => SupportOriginId::Standalone,
        CandidateSource::Stabilization => SupportOriginId::Island,
        CandidateSource::Reinforcement => SupportOriginId::Reinforcement,
    }
}

/// The host a gridless merge aims at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeHost {
    pub host_type_id: SupportTypeId,
    pub host_id: String,
    pub tip_pos: Vec3,
}

/// The closest existing host (its tip, or a shaft joint) within the merge
/// reach, measured in plan and ranked on the 3D span plus a load penalty for
/// hosts already carrying long members. Near-plate hosts never host merges.
pub fn find_merge_host(tip_pos: Vec3, model_id: &str, draft: &SupportState) -> Option<MergeHost> {
    let reach2 = GRIDLESS_MERGE_RADIUS_MM * GRIDLESS_MERGE_RADIUS_MM;
    let mut best: Option<MergeHost> = None;
    let mut best_score = f64::INFINITY;
    let mut load_of: HashMap<String, f64> = HashMap::new();
    let mut score_for = |host_type_id: SupportTypeId, host_id: &str, adjusted_d2: f64| {
        let key = format!("{host_type_id}:{host_id}");
        let lmax = *load_of
            .entry(key)
            .or_insert_with(|| max_member_span_mm(host_type_id, host_id, draft));
        js_max(0.0, adjusted_d2).sqrt() + MERGE_HOST_LOAD_WEIGHT * lmax
    };
    let near_plate = near_plate_origin();

    for HostEntry {
        host_type_id,
        host_id,
        entity,
    } in collect_host_entities(draft)
    {
        if entity.base.model_id != model_id {
            continue;
        }
        if entity.base.origin == Some(near_plate) {
            continue;
        }
        if let Some(cone) = &entity.contact_cone {
            let tp = cone.pos;
            let dx = tip_pos.x - tp.x;
            let dy = tip_pos.y - tp.y;
            if dx * dx + dy * dy <= reach2 {
                let dz = tip_pos.z - tp.z;
                let score = score_for(host_type_id, host_id, dx * dx + dy * dy + dz * dz);
                if score < best_score {
                    best_score = score;
                    best = Some(MergeHost {
                        host_type_id,
                        host_id: host_id.to_owned(),
                        tip_pos: tp,
                    });
                }
            }
        }
        for seg in &entity.segments {
            let Some(jp) = seg
                .bottom_joint
                .as_ref()
                .or(seg.top_joint.as_ref())
                .map(|j| j.pos)
            else {
                continue;
            };
            let dx = tip_pos.x - jp.x;
            let dy = tip_pos.y - jp.y;
            if dx * dx + dy * dy > reach2 {
                continue;
            }
            let dz = tip_pos.z - jp.z;
            let adjusted = (dx * dx + dy * dy + dz * dz) * 0.9;
            let score = score_for(host_type_id, host_id, adjusted);
            if score < best_score {
                best_score = score;
                best = Some(MergeHost {
                    host_type_id,
                    host_id: host_id.to_owned(),
                    tip_pos: jp,
                });
            }
        }
    }
    best
}

/// The parts of a hosted branch's build input the caller chooses.
pub struct HostedBranchInput<'k> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'k str,
    pub parent_knot: &'k Knot,
    pub tip_contact_diameter_mm: Option<f64>,
    pub roots_diameter_mm: Option<f64>,
}

/// Build a hosted branch at the host-relative floor, retrying at the profile
/// band when the thicker member is refused by `rejected` (the clearance gate).
pub fn build_hosted_branch(
    input: &HostedBranchInput<'_>,
    band_shaft_mm: f64,
    host_diameter_mm: f64,
    ctx: &PlanCtx<'_>,
    rejected: &dyn Fn(&BranchBuildResult) -> bool,
) -> Option<BranchBuildResult> {
    let host_ratio = ctx.live().auto_support.member_host_shaft_ratio;
    // SoulCrafted (#360): a zone size's trunk is its band, so a member at
    // the band would be as thick as its host and its tilted end would stick
    // out of it; it takes its share of the host instead.
    let band_shaft_mm = if ctx.live().auto_support.zone_rules.is_some() {
        band_shaft_mm.min(host_diameter_mm * host_ratio)
    } else {
        band_shaft_mm
    };
    let floored_shaft_mm = member_shaft_diameter_mm(band_shaft_mm, host_diameter_mm, host_ratio);
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
    // A branch whose tip cone goes through the model is no branch (#500).
    let rejected = |built: &BranchBuildResult| {
        rejected(built)
            || built
                .branch
                .contact_cone
                .as_ref()
                .is_some_and(|cone| ctx.cone_crosses(cone))
    };
    let floored = build(floored_shaft_mm);
    if !rejected(&floored) {
        return Some(floored);
    }
    if floored_shaft_mm <= band_shaft_mm {
        return None;
    }
    let at_band = build(band_shaft_mm);
    (!rejected(&at_band)).then_some(at_band)
}

/// One sample of a host shaft, for the fan.
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

/// Pick the fanning host for an uncovered island: the nearest shaft point, but
/// grid hosts only up close.
pub fn pick_fan_host<'p>(
    shaft_points: &'p [FanShaftPoint],
    grid_host_ids: &HashSet<String>,
    target: Vec3,
    fan_radius_mm: f64,
    grid_host_fan_radius_mm: f64,
) -> Option<(&'p FanShaftPoint, f64)> {
    let mut best: Option<&FanShaftPoint> = None;
    let mut best_d2 = f64::INFINITY;
    let mut best_regular: Option<&FanShaftPoint> = None;
    let mut best_regular_d2 = f64::INFINITY;
    for sp in shaft_points {
        let dx = target.x - sp.pos.x;
        let dy = target.y - sp.pos.y;
        let dz = target.z - sp.pos.z;
        let d2 = dx * dx + dy * dy + dz * dz;
        if d2 < best_d2 {
            best_d2 = d2;
            best = Some(sp);
        }
        if !grid_host_ids.contains(&sp.host_id) && d2 < best_regular_d2 {
            best_regular_d2 = d2;
            best_regular = Some(sp);
        }
    }
    if let Some(b) = best
        && grid_host_ids.contains(&b.host_id)
        && best_d2 > grid_host_fan_radius_mm * grid_host_fan_radius_mm
    {
        best = best_regular;
        best_d2 = best_regular_d2;
    }
    let b = best?;
    (best_d2 <= fan_radius_mm * fan_radius_mm).then_some((b, best_d2))
}

fn clamp01(v: f64) -> f64 {
    js_min(1.0, js_max(0.0, v))
}

/// Squared distance between two 3D segments (closest points).
pub fn segment_distance_sq(p1: Vec3, p2: Vec3, p3: Vec3, p4: Vec3) -> f64 {
    let (d1x, d1y, d1z) = (p2.x - p1.x, p2.y - p1.y, p2.z - p1.z);
    let (d2x, d2y, d2z) = (p4.x - p3.x, p4.y - p3.y, p4.z - p3.z);
    let (rx, ry, rz) = (p1.x - p3.x, p1.y - p3.y, p1.z - p3.z);
    let a = d1x * d1x + d1y * d1y + d1z * d1z;
    let e = d2x * d2x + d2y * d2y + d2z * d2z;
    let f = d2x * rx + d2y * ry + d2z * rz;
    let mut s = 0.0;
    let mut t = 0.0;
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

/// Whether the straight leaf path (knot → tip) passes within the leaf radius +
/// shaft radius of any other host's shaft.
pub fn leaf_path_crosses_supports(
    knot_pos: Vec3,
    tip_pos: Vec3,
    leaf_radius_mm: f64,
    draft: &SupportState,
    host_id: Option<&str>,
) -> bool {
    for HostEntry {
        host_id: candidate_id,
        entity,
        ..
    } in collect_host_entities(draft)
    {
        if Some(candidate_id) == host_id {
            continue;
        }
        for seg in &entity.segments {
            let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
                continue;
            };
            let clearance = leaf_radius_mm + seg.diameter / 2.0;
            // SoulCrafted (#445): further apart than the clearance along one
            // axis is further apart than that, and most shafts are.
            let apart = |p: f64, q: f64, r: f64, s: f64| {
                p.min(q) > r.max(s) + clearance || r.min(s) > p.max(q) + clearance
            };
            if apart(knot_pos.x, tip_pos.x, a.pos.x, b.pos.x)
                || apart(knot_pos.y, tip_pos.y, a.pos.y, b.pos.y)
                || apart(knot_pos.z, tip_pos.z, a.pos.z, b.pos.z)
            {
                continue;
            }
            if segment_distance_sq(knot_pos, tip_pos, a.pos, b.pos) < clearance * clearance {
                return true;
            }
        }
    }
    false
}

/// Shaft samples per segment for fanning host picking.
const SHAFT_SAMPLES_PER_SEGMENT: u32 = 10;
/// Max leaf-fanning convergence passes.
pub const MAX_FANNING_PASSES: usize = 5;

/// The fanning host pool: every host segment with both joints, sampled 11
/// times. Near-plate hosts never host fan leaves.
pub fn collect_fan_shaft_points(draft: &SupportState) -> Vec<FanShaftPoint> {
    shaft_points_of(draft, |_, _| true)
}

/// [`collect_fan_shaft_points`] of the segments that pass within `reach_mm` of
/// `target` on the plate, in the same order: a sample further than that on the
/// plate is out of a fan's reach. Sampling every shaft for each candidate is
/// what grows with the square of the supports.
pub fn collect_fan_shaft_points_near(
    draft: &SupportState,
    target: Vec3,
    reach_mm: f64,
) -> Vec<FanShaftPoint> {
    let apart = |a: f64, b: f64, at: f64| at < a.min(b) - reach_mm || at > a.max(b) + reach_mm;
    shaft_points_of(draft, |start, end| {
        !apart(start.x, end.x, target.x) && !apart(start.y, end.y, target.y)
    })
}

/// The samples of the shafts of `draft` whose segments `keep` takes.
fn shaft_points_of(draft: &SupportState, keep: impl Fn(Vec3, Vec3) -> bool) -> Vec<FanShaftPoint> {
    let near_plate = near_plate_origin();
    let mut points = Vec::new();
    for HostEntry {
        host_type_id,
        host_id,
        entity,
    } in collect_host_entities(draft)
    {
        if entity.base.origin == Some(near_plate) {
            continue;
        }
        for seg in &entity.segments {
            let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
                continue;
            };
            let (start, end) = (a.pos, b.pos);
            if !keep(start, end) {
                continue;
            }
            for i in 0..=SHAFT_SAMPLES_PER_SEGMENT {
                let t = f64::from(i) / f64::from(SHAFT_SAMPLES_PER_SEGMENT);
                points.push(FanShaftPoint {
                    host_type_id,
                    host_id: host_id.to_owned(),
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
    points
}

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

/// The world-space span of a host segment — the line a hosted member's knot
/// has to lie on. The ONE resolver for it: the merge knot search walks this
/// line and the orphan cull measures drift against it. A bottom segment starts
/// at the root top; a top segment ends at the contact cone.
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

/// The shaft a knot sits on, across every type that has segments. A legacy
/// knot keyed to its owner's id reads as missing.
pub fn find_host_segment<'d>(
    draft: &'d SupportState,
    parent_shaft_id: &str,
) -> Option<(SupportTypeId, &'d str, &'d Segment)> {
    let c = &draft.collections;
    macro_rules! scan {
        ($type_id:expr, $collection:expr) => {
            if get_support_type_descriptor($type_id).has_segments {
                for (owner_id, entity) in $collection.iter() {
                    if let Some(seg) = entity.segments.iter().find(|s| s.id == parent_shaft_id) {
                        return Some(($type_id, owner_id.as_str(), seg));
                    }
                    if owner_id == parent_shaft_id {
                        return None;
                    }
                }
            }
        };
    }
    scan!(SupportTypeId::Trunk, c.trunks);
    scan!(SupportTypeId::Branch, c.branches);
    scan!(SupportTypeId::Twig, c.twigs);
    scan!(SupportTypeId::Stick, c.sticks);
    scan!(SupportTypeId::Stump, c.stumps);
    scan!(SupportTypeId::Kickstand, c.kickstands);
    None
}

/// Rehost knots whose `parentShaftId` is a host entity id (legacy) to the
/// nearest segment of that host.
pub fn rehost_legacy_knots(draft: &SupportState) -> SupportState {
    let host_by_id: HashMap<&str, &Trunk> = collect_host_entities(draft)
        .into_iter()
        .map(|h| (h.host_id, h.entity))
        .collect();
    let mut out: Option<SupportState> = None;
    for (kid, knot) in &draft.collections.knots {
        let Some(host) = host_by_id.get(knot.parent_shaft_id.as_str()) else {
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
            let next = out.get_or_insert_with(|| draft.clone());
            let k = next.collections.knots.get_mut(kid).expect("knot");
            k.parent_shaft_id = seg.id.clone();
            k.t = Some(t);
        }
    }
    out.unwrap_or_else(|| draft.clone())
}

/// Enforce "contact cone body == the shaft it sits on" across the draft (the
/// resize thickens shafts after cones are built). Only the body moves.
pub fn sync_contact_cone_diameters(draft: &SupportState) -> SupportState {
    sync_contact_cone_diameters_with(draft, 1.0)
}

/// [`sync_contact_cone_diameters`] with a leaf's cone body at
/// `leaf_body_ratio` of its host segment (SoulCrafted, #360): a zone size's
/// leaf narrower than its host, so its tilted base never sticks out of it.
pub fn sync_contact_cone_diameters_with(
    draft: &SupportState,
    leaf_body_ratio: f64,
) -> SupportState {
    let c = &draft.collections;
    let mut diameter_by_segment: HashMap<String, f64> = HashMap::new();
    let follows = |t: SupportTypeId| {
        matches!(
            get_support_type_descriptor(t).cone_body_follows,
            Some(ConeBodyFollows::OwnFirstSegment | ConeBodyFollows::OwnLastSegment)
        )
    };
    let mut add = |segments: &[Segment]| {
        for seg in segments {
            if seg.diameter > 0.0 {
                diameter_by_segment.insert(seg.id.clone(), seg.diameter);
            }
        }
    };
    // Registry order: trunk, branch, leaf, twig, stick, brace, stump, kickstand.
    if follows(SupportTypeId::Trunk) {
        c.trunks.values().for_each(|e| add(&e.segments));
    }
    if follows(SupportTypeId::Branch) {
        c.branches.values().for_each(|e| add(&e.segments));
    }
    if follows(SupportTypeId::Twig) {
        c.twigs.values().for_each(|e| add(&e.segments));
    }
    if follows(SupportTypeId::Stick) {
        c.sticks.values().for_each(|e| add(&e.segments));
    }
    if follows(SupportTypeId::Stump) {
        c.stumps.values().for_each(|e| add(&e.segments));
    }
    if follows(SupportTypeId::Kickstand) {
        c.kickstands.values().for_each(|e| add(&e.segments));
    }

    let mut changed = false;
    let mut sync = |cone: &mut ContactCone, host_dia: f64| {
        if (cone.profile.body_diameter_mm - host_dia).abs() > 1e-6 {
            changed = true;
            cone.profile.body_diameter_mm = host_dia;
        }
    };
    let mut next = draft.clone();
    for t in next.collections.trunks.values_mut() {
        let top = t.segments.last().map(|s| s.diameter);
        if let (Some(cone), Some(d)) = (t.contact_cone.as_mut(), top) {
            sync(cone, d);
        }
    }
    for l in next.collections.leaves.values_mut() {
        let Some(knot) = draft.collections.knots.get(&l.parent_knot_id) else {
            continue;
        };
        let Some(&d) = diameter_by_segment.get(knot.parent_shaft_id.as_str()) else {
            continue;
        };
        sync(&mut l.contact_cone, d * leaf_body_ratio);
    }
    for b in next.collections.branches.values_mut() {
        let first = b.segments.first().map(|s| s.diameter);
        if let (Some(cone), Some(d)) = (b.contact_cone.as_mut(), first) {
            sync(cone, d);
        }
    }
    if changed { next } else { draft.clone() }
}

/// One orphaned leaf/branch, or a culled host.
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
}

/// A shaft-hosted member as the cull reads it.
struct MemberView<'d> {
    id: &'d str,
    knot_id: &'d str,
    cone: Option<&'d ContactCone>,
    segments: &'d [Segment],
}

fn member_views(draft: &SupportState, type_id: SupportTypeId) -> Vec<MemberView<'_>> {
    let c = &draft.collections;
    match type_id {
        SupportTypeId::Leaf => c
            .leaves
            .iter()
            .map(|(id, l)| MemberView {
                id,
                knot_id: &l.parent_knot_id,
                cone: Some(&l.contact_cone),
                segments: &[],
            })
            .collect(),
        SupportTypeId::Branch => c
            .branches
            .iter()
            .map(|(id, b)| MemberView {
                id,
                knot_id: &b.parent_knot_id,
                cone: b.contact_cone.as_ref(),
                segments: &b.segments,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Validate hosted members' attachment after the forest resize; cull orphans
/// (and hosts whose shafts pierce the mesh) and return them for reporting.
///
/// SoulCrafted (#500): with `cones_clear`, a support whose tip cone goes
/// through the model on its way to its contact ([`contact_cone_crosses_model`])
/// is culled too: a trunk as a blocked host (its members are placed again), a
/// leaf or a branch as blocked, a stick or a stump by itself.
pub fn validate_and_cull_orphans(
    draft: &SupportState,
    mesh: Option<&PlanMesh<'_>>,
    cones_clear: bool,
) -> (SupportState, Vec<OrphanInfo>) {
    let mut orphans: Vec<OrphanInfo> = Vec::new();
    const DRIFT_TOL_SQ: f64 = 0.25; // 0.5mm
    let mut hosts_to_remove: IndexSet<String> = IndexSet::new();
    let mut culled_host_type_name: HashMap<String, &'static str> = HashMap::new();

    // Host shafts that pierce the mesh: cull the whole pillar.
    if let Some(mesh) = mesh {
        for HostEntry {
            host_type_id,
            host_id,
            entity,
        } in collect_host_entities(draft)
        {
            let mut blocked = false;
            for seg in &entity.segments {
                let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
                    continue;
                };
                let (start, end) = (a.pos, b.pos);
                let radius = seg.diameter / 2.0;
                let tip = entity.contact_cone.as_ref().map_or(end, |c| c.pos);
                let check_end = Vec3::new(
                    end.x * 0.9 + tip.x * 0.1,
                    end.y * 0.9 + tip.y * 0.1,
                    end.z * 0.9 + tip.z * 0.1,
                );
                if mesh.shaft_blocked(start, check_end, radius + 0.15) {
                    blocked = true;
                    break;
                }
            }
            let crossing = !blocked
                && cones_clear
                && entity
                    .contact_cone
                    .as_ref()
                    .is_some_and(|c| contact_cone_crosses_model(c, mesh));
            if blocked || crossing {
                let where_ = entity.contact_cone.as_ref().map_or(String::new(), |c| {
                    format!(
                        " @ ({}, {}, Z{})",
                        crate::three::math_utils::js_to_fixed(c.pos.x, 1),
                        crate::three::math_utils::js_to_fixed(c.pos.y, 1),
                        crate::three::math_utils::js_to_fixed(c.pos.z, 1)
                    )
                });
                let type_name = get_support_type_descriptor(host_type_id).singular;
                culled_host_type_name.insert(host_id.to_owned(), type_name);
                orphans.push(OrphanInfo {
                    id: host_id.to_owned(),
                    kind: host_type_id,
                    reason: OrphanReason::HostBlocked,
                    host_id: None,
                    knot_id: None,
                    detail: Some(if crossing {
                        format!("{type_name} tip cone crosses mesh{where_}")
                    } else {
                        format!("{type_name} shaft pierces mesh{where_}")
                    }),
                });
                hosts_to_remove.insert(host_id.to_owned());
            }
        }
    }

    let host_ids: HashSet<&str> = collect_host_entities(draft)
        .iter()
        .map(|h| h.host_id)
        .collect();
    let fixed1 = |v: f64| crate::three::math_utils::js_to_fixed(v, 1);
    let check_attachment = |kind: SupportTypeId,
                            member: &MemberView<'_>,
                            orphans: &mut Vec<OrphanInfo>|
     -> bool {
        let push = |orphans: &mut Vec<OrphanInfo>,
                    reason,
                    host: Option<&str>,
                    knot: Option<&str>,
                    detail: String| {
            orphans.push(OrphanInfo {
                id: member.id.to_owned(),
                kind,
                reason,
                host_id: host.map(str::to_owned),
                knot_id: knot.map(str::to_owned),
                detail: Some(detail),
            });
        };
        if member.knot_id.is_empty() {
            push(
                orphans,
                OrphanReason::MissingKnot,
                None,
                None,
                "no parentKnotId".to_owned(),
            );
            return false;
        }
        let Some(knot) = draft.collections.knots.get(member.knot_id) else {
            push(
                orphans,
                OrphanReason::MissingKnot,
                None,
                Some(member.knot_id),
                "knot not in draft".to_owned(),
            );
            return false;
        };
        let Some((host_type_id, owner_id, seg)) = find_host_segment(draft, &knot.parent_shaft_id)
        else {
            let reason = if host_ids.contains(knot.parent_shaft_id.as_str()) {
                OrphanReason::MissingSegment
            } else {
                OrphanReason::MissingHost
            };
            push(
                orphans,
                reason,
                Some(&knot.parent_shaft_id),
                Some(&knot.id),
                format!("knot {} parent {}", knot.id, knot.parent_shaft_id),
            );
            return false;
        };
        let owner = host_entity_of(draft, host_type_id, owner_id);
        let Some((start, end)) = owner.and_then(|o| host_segment_span(draft, o, seg)) else {
            let seg_index = owner.and_then(|o| o.segments.iter().position(|s| s.id == seg.id));
            let seg_num = seg_index.map_or(0, |i| i + 1);
            let total = owner.map_or(0, |o| o.segments.len());
            let origin = owner
                .and_then(|o| o.base.origin)
                .map_or("unset", SupportOriginId::as_str);
            let short: String = seg.id.chars().take(8).collect();
            push(
                orphans,
                OrphanReason::MissingHost,
                Some(owner_id),
                Some(&knot.id),
                format!(
                    "segment missing joints (seg {short}, topJoint {}, bottomJoint {}, seg {seg_num}/{total}, origin {origin})",
                    if seg.top_joint.is_some() { "yes" } else { "no" },
                    if seg.bottom_joint.is_some() {
                        "yes"
                    } else {
                        "no"
                    },
                ),
            );
            return false;
        };
        let drift2 = point_to_segment_distance_sq(knot.pos, start, end);
        if drift2 > DRIFT_TOL_SQ {
            push(
                orphans,
                OrphanReason::Drift,
                Some(owner_id),
                Some(&knot.id),
                format!(
                    "drift {}mm from shaft",
                    crate::three::math_utils::js_to_fixed(drift2.sqrt(), 2)
                ),
            );
            return false;
        }
        if let Some(cone) = member.cone {
            let tip_pos = cone.pos;
            let mut blocked = false;
            if !get_support_type_descriptor(kind).has_segments {
                if let Some(mesh) = mesh {
                    blocked = contact_cone_collides(
                        knot.pos,
                        cone.pos,
                        cone.normal,
                        cone.surface_normal,
                        mesh,
                    );
                }
            } else if let Some(mesh) = mesh {
                blocked = branch_collides_with_sdf(member.segments, mesh);
            }
            if blocked {
                push(
                    orphans,
                    OrphanReason::Blocked,
                    Some(owner_id),
                    Some(&knot.id),
                    "knot→tip blocked by mesh".to_owned(),
                );
                return false;
            }
            if cones_clear && mesh.is_some_and(|m| contact_cone_crosses_model(cone, m)) {
                push(
                    orphans,
                    OrphanReason::Blocked,
                    Some(owner_id),
                    Some(&knot.id),
                    "tip cone crosses mesh".to_owned(),
                );
                return false;
            }
            // Shallow leaf/branch guard: a permissive 75° ceiling after resize.
            let v_dist = tip_pos.z - knot.pos.z;
            let h_dist = js_hypot(&[tip_pos.x - knot.pos.x, tip_pos.y - knot.pos.y]);
            let angle_deg =
                (js_atan2(h_dist, js_max(0.001, v_dist)) * 180.0) / std::f64::consts::PI;
            let max_angle = 75.0;
            if v_dist < 0.4 || angle_deg > max_angle + 1e-6 {
                push(
                    orphans,
                    OrphanReason::Blocked,
                    Some(owner_id),
                    Some(&knot.id),
                    format!(
                        "too shallow {}° > {max_angle}° (h={} v={})",
                        fixed1(angle_deg),
                        fixed1(h_dist),
                        fixed1(v_dist)
                    ),
                );
                return false;
            }
            if leaf_path_crosses_supports(knot.pos, tip_pos, 0.25, draft, Some(owner_id)) {
                push(
                    orphans,
                    OrphanReason::Cross,
                    Some(owner_id),
                    Some(&knot.id),
                    "leaf/branch crosses another shaft after thickening".to_owned(),
                );
                return true;
            }
        }
        true
    };

    // Registry walk order: leaf orphans before branch ones.
    let mut members_to_remove: IndexMap<SupportTypeId, IndexSet<String>> = IndexMap::new();
    for type_id in SHAFT_HOSTED_MEMBER_TYPE_ORDER {
        let removal = members_to_remove.entry(type_id).or_default();
        for member in member_views(draft, type_id) {
            if !check_attachment(type_id, &member, &mut orphans) {
                removal.insert(member.id.to_owned());
            }
        }
    }
    // Members whose host was culled are orphaned too.
    for type_id in SHAFT_HOSTED_MEMBER_TYPE_ORDER {
        for member in member_views(draft, type_id) {
            if members_to_remove[&type_id].contains(member.id) {
                continue;
            }
            let Some(knot) = draft.collections.knots.get(member.knot_id) else {
                continue;
            };
            let culled_host_id = find_host_segment(draft, &knot.parent_shaft_id)
                .map_or(knot.parent_shaft_id.as_str(), |(_, id, _)| id);
            if let Some(name) = culled_host_type_name.get(culled_host_id) {
                orphans.push(OrphanInfo {
                    id: member.id.to_owned(),
                    kind: type_id,
                    reason: OrphanReason::MissingHost,
                    host_id: Some(culled_host_id.to_owned()),
                    knot_id: Some(knot.id.clone()),
                    detail: Some(format!("{name} culled (blocked)")),
                });
                members_to_remove
                    .get_mut(&type_id)
                    .expect("type")
                    .insert(member.id.to_owned());
            }
        }
    }

    // Sticks and stumps whose tip cone crosses the model (#500): they hold
    // nothing else, so they go by themselves.
    let mut sticks_to_remove: HashSet<String> = HashSet::new();
    let mut stumps_to_remove: HashSet<String> = HashSet::new();
    if cones_clear && let Some(mesh) = mesh {
        let crossing_detail = |name: &str, cone: &ContactCone| {
            format!(
                "{name} tip cone crosses mesh @ ({}, {}, Z{})",
                fixed1(cone.pos.x),
                fixed1(cone.pos.y),
                fixed1(cone.pos.z)
            )
        };
        for (id, stick) in &draft.collections.sticks {
            let crossing = [&stick.contact_cone_a, &stick.contact_cone_b]
                .into_iter()
                .find(|c| contact_cone_crosses_model(c, mesh));
            if let Some(cone) = crossing {
                orphans.push(OrphanInfo {
                    id: id.clone(),
                    kind: SupportTypeId::Stick,
                    reason: OrphanReason::Blocked,
                    host_id: None,
                    knot_id: None,
                    detail: Some(crossing_detail("Stick", cone)),
                });
                sticks_to_remove.insert(id.clone());
            }
        }
        for (id, stump) in &draft.collections.stumps {
            if contact_cone_crosses_model(&stump.contact_cone, mesh) {
                orphans.push(OrphanInfo {
                    id: id.clone(),
                    kind: SupportTypeId::Stump,
                    reason: OrphanReason::Blocked,
                    host_id: None,
                    knot_id: None,
                    detail: Some(crossing_detail("Stump", &stump.contact_cone)),
                });
                stumps_to_remove.insert(id.clone());
            }
        }
    }

    if members_to_remove.values().all(IndexSet::is_empty)
        && hosts_to_remove.is_empty()
        && sticks_to_remove.is_empty()
        && stumps_to_remove.is_empty()
    {
        return (draft.clone(), orphans);
    }

    // Knots used only by culled members.
    let mut remaining_knot_users: HashMap<String, usize> = HashMap::new();
    for type_id in SHAFT_HOSTED_MEMBER_TYPE_ORDER {
        for member in member_views(draft, type_id) {
            if members_to_remove[&type_id].contains(member.id) {
                continue;
            }
            if !member.knot_id.is_empty() {
                *remaining_knot_users
                    .entry(member.knot_id.to_owned())
                    .or_default() += 1;
            }
        }
    }
    // Brace knots are never culled.
    for brace in draft.collections.braces.values() {
        *remaining_knot_users
            .entry(brace.start_knot_id.clone())
            .or_default() += 1;
        *remaining_knot_users
            .entry(brace.end_knot_id.clone())
            .or_default() += 1;
    }
    let mut knots_to_remove: HashSet<String> = HashSet::new();
    for type_id in SHAFT_HOSTED_MEMBER_TYPE_ORDER {
        let views = member_views(draft, type_id);
        for id in &members_to_remove[&type_id] {
            if let Some(member) = views.iter().find(|m| m.id == id)
                && !member.knot_id.is_empty()
                && !remaining_knot_users.contains_key(member.knot_id)
            {
                knots_to_remove.insert(member.knot_id.to_owned());
            }
        }
    }
    // Knots on removed hosts.
    let mut segment_ids_to_remove: HashSet<&str> = HashSet::new();
    for id in &hosts_to_remove {
        if let Some(entity) = draft.collections.trunks.get(id) {
            for seg in &entity.segments {
                segment_ids_to_remove.insert(&seg.id);
            }
        }
    }
    for (kid, knot) in &draft.collections.knots {
        if (segment_ids_to_remove.contains(knot.parent_shaft_id.as_str())
            || hosts_to_remove.contains(&knot.parent_shaft_id))
            && !remaining_knot_users.contains_key(kid)
        {
            knots_to_remove.insert(kid.clone());
        }
    }

    let mut next = draft.clone();
    let c = &mut next.collections;
    c.leaves
        .retain(|id, _| !members_to_remove[&SupportTypeId::Leaf].contains(id));
    c.branches
        .retain(|id, _| !members_to_remove[&SupportTypeId::Branch].contains(id));
    c.knots.retain(|id, _| !knots_to_remove.contains(id));
    c.sticks.retain(|id, _| !sticks_to_remove.contains(id));
    c.stumps.retain(|id, _| !stumps_to_remove.contains(id));
    let removed_roots: Vec<String> = hosts_to_remove
        .iter()
        .filter_map(|id| draft.collections.trunks.get(id).map(|t| t.root_id.clone()))
        .collect();
    c.trunks.retain(|id, _| !hosts_to_remove.contains(id));
    for root_id in removed_roots {
        if root_id.is_empty() || !c.roots.contains_key(&root_id) {
            continue;
        }
        // DragonFruit looks in the grid-host collections only (the trunks).
        let still_used = c.trunks.values().any(|t| t.root_id == root_id);
        if !still_used {
            c.roots.shift_remove(&root_id);
        }
    }
    (next, orphans)
}

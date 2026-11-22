//! Where a built trunk lands on the pillars' grid: on its node as a new
//! pillar, hung from a pillar already standing there or on a node nearby as
//! a leaf or a branch, or refused.
//!
//! `settings` are the run's own for this decision; `live` the ones the
//! member builders read. Ids come from `new_id`; `mesh` is the model's
//! collision field.

use indexmap::IndexMap;
use serde::Serialize;

use super::grid_math::{grid_node_key_from_xy, grid_snapped_xy_from_key, key_indices, node_key};
use crate::placement_logic::smart_placement_search_utils::{
    SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG, SOCKET_ELBOW_MAX_ANGLE_FROM_VERTICAL_DEG,
    TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG, distance_3d,
    get_length_aware_max_angle_from_vertical_deg, member_departure_angle_from_vertical_deg,
};
use crate::support_types::branch::branch_builder::{BranchBuildInput, build_branch_data};
use crate::support_types::leaf::leaf_builder::{LeafBuildInput, build_leaf_data};
use crate::support_types::stump::stump_auto_placement::{
    ContactOverrideRequest, build_stump_contact_override,
};
use crate::support_types::trunk::trunk_builder::TrunkBuildResult;
use crate::support_types::trunk::trunk_route_resolution::{
    get_default_snapped_validity, get_resolved_snapped_node_key, get_resolved_snapped_root_pos,
    get_resolved_snapped_validity, has_resolved_snapped_root,
};
use crate::support_types::trunk::trunk_route_types::{
    SnappedRoute, SnappedRouteValidity, TrunkRouteValidity,
};
use support_geom::geom::math_utils::{js_acos, js_hypot, js_max, js_min, rad_to_deg};
use support_model::auto_support::constants::MAX_AUTO_LEAF_SPAN_MM;
use support_model::collision_avoidance::{CollisionField, is_shaft_blocked};
use support_model::constants::member_shaft_diameter_mm;
use support_model::rendering::support_builder::SupportData;
use support_model::settings::types::SupportSettings;
use support_model::shafts::calculate_knot_position_on_segment_from_t;
use support_model::support_kinds::{
    CONTACT_OVERRIDE_REGISTRATIONS, HostRef, PlacedSupport, PlacementPrimitive, SupportEntityValue,
    SupportPlacementMetric, SupportPlacementSettingPath, SupportTypeId, grid_host_types,
    placement_of_resolved, select_type_for_placement,
};
use support_model::tips::get_final_socket_position;
use support_model::types::{Joint, Knot, LimitationCode, Roots, SupportState, Trunk, Vec3};

/// The clearance a new pillar keeps from the model past its radius, mm: the
/// same the final cull holds a trunk to, so a pillar placed here survives it.
const TRUNK_CLEARANCE_MM: f64 = 0.15;
/// The node key of a placement made with the grid off.
const GRID_OFF_KEY: &str = "disabled";
/// The node key of a pillar the router already placed off the grid.
const OFF_GRID_KEY: &str = "unsnapped";
/// How many rings of nodes a tip looks through for a pillar to hang from.
const NEIGHBOR_RINGS: i64 = 3;

/// Why the grid refused a contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GridPlacementRejectReason {
    KnotAboveTip,
    StumpBelowRoot,
    NoHostSegment,
    ModelMismatch,
    NoValidAttachment,
    CollisionWithModel,
    OutOfBounds,
}

impl GridPlacementRejectReason {
    /// A support type's own refusal, as the grid reports it.
    fn from_limitation(code: LimitationCode) -> Self {
        match code {
            LimitationCode::KnotAboveTip => Self::KnotAboveTip,
            LimitationCode::StumpBelowRoot => Self::StumpBelowRoot,
            LimitationCode::OutOfBounds => Self::OutOfBounds,
            LimitationCode::CollisionWithModel => Self::CollisionWithModel,
            LimitationCode::AngleTooSteep | LimitationCode::TooCloseToExisting => {
                Self::NoValidAttachment
            }
        }
    }
}

/// What the grid made of a contact.
// Built once and moved to the caller: boxing buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GridPlacementDecision {
    /// A support placed on the contact.
    #[serde(rename_all = "camelCase")]
    Place {
        /// The node it landed on; empty when a type built it its own way.
        node_key: String,
        placed: PlacedSupport,
        #[serde(skip_serializing_if = "Option::is_none")]
        support_data: Option<SupportData>,
    },
    #[serde(rename_all = "camelCase")]
    Reject {
        node_key: String,
        reason: GridPlacementRejectReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        trunk_build: Option<Box<TrunkBuildResult>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        support_data: Option<SupportData>,
    },
}

/// What a grid decision is made from.
#[derive(Debug)]
pub struct DecideGridPlacementArgs<'a> {
    pub settings: &'a SupportSettings,
    pub snapshot: &'a SupportState,
    /// The trunk built for the contact, before the grid had its say.
    pub candidate: &'a TrunkBuildResult,
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    pub mesh: Option<&'a dyn CollisionField>,
}

/// A pillar a contact may hang from.
#[derive(Clone, Copy)]
struct Pillar<'a> {
    type_id: SupportTypeId,
    id: &'a str,
    trunk: &'a Trunk,
    root: &'a Roots,
}

impl Pillar<'_> {
    fn host(&self) -> Option<HostRef> {
        Some(HostRef {
            type_id: self.type_id,
            id: self.id.to_owned(),
        })
    }
}

/// The pillars on each node: by where each stands, then by where its
/// contact is (a pillar routed off the grid holds its contact's node too).
fn pillars_by_node<'a>(
    pillars: &[Pillar<'a>],
    spacing_mm: f64,
) -> IndexMap<String, Vec<Pillar<'a>>> {
    let mut nodes: IndexMap<String, Vec<Pillar<'a>>> = IndexMap::new();
    let mut add = |key: String, pillar: Pillar<'a>| {
        let on = nodes.entry(key).or_default();
        if !on.iter().any(|p| p.id == pillar.id) {
            on.push(pillar);
        }
    };
    for p in pillars {
        let at = p.root.transform.pos;
        add(grid_node_key_from_xy(at.x, at.y, spacing_mm), *p);
    }
    for p in pillars {
        if let Some(cone) = &p.trunk.contact_cone {
            add(
                grid_node_key_from_xy(cone.pos.x, cone.pos.y, spacing_mm),
                *p,
            );
        }
    }
    nodes
}

/// The pillars standing (or touching the model) within half a step of the
/// node of `key`, nearest first.
fn pillars_near<'a>(pillars: &[Pillar<'a>], key: &str, spacing_mm: f64) -> Vec<Pillar<'a>> {
    let (cx, cy) = grid_snapped_xy_from_key(key, spacing_mm);
    let mut near: Vec<(Pillar<'a>, f64)> = pillars
        .iter()
        .filter_map(|p| {
            let at = p.root.transform.pos;
            let mut d = js_hypot(&[at.x - cx, at.y - cy]);
            if let Some(cone) = &p.trunk.contact_cone {
                d = js_min(d, js_hypot(&[cone.pos.x - cx, cone.pos.y - cy]));
            }
            (d <= spacing_mm * 0.5).then_some((*p, d))
        })
        .collect();
    near.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    near.into_iter().map(|(p, _)| p).collect()
}

/// What every search for a pillar to hang from shares.
struct Hanging<'a, 'b> {
    tip_pos: Vec3,
    tip_normal: Vec3,
    model_id: &'a str,
    min_angle_deg: f64,
    settings: &'a SupportSettings,
    live: &'a SupportSettings,
    step_mm: f64,
    mesh: Option<&'a dyn CollisionField>,
    new_id: &'b mut dyn FnMut() -> String,
}

/// The first of `pillars` that can take the contact, hung on it.
fn hang_on_any(
    pillars: &[Pillar<'_>],
    key: &str,
    h: &mut Hanging<'_, '_>,
    occupied: bool,
) -> Option<GridPlacementDecision> {
    pillars
        .iter()
        .filter(|p| !p.trunk.segments.is_empty())
        .find_map(|p| hang_on(key, p, h, occupied))
}

/// The candidate with its snapped route set; an error replaces the route's
/// and clears the warnings.
fn with_snap(
    candidate: &TrunkBuildResult,
    root: Vec3,
    key: Option<String>,
    snapped: SnappedRouteValidity,
    validity: Option<TrunkRouteValidity>,
    error: Option<LimitationCode>,
) -> TrunkBuildResult {
    let error = error.or(candidate.route.route.error);
    let mut out = candidate.clone();
    out.route.snapped = Some(SnappedRoute {
        snapped_root_pos: root,
        snapped_node_key: key,
        snapped_validity: snapped,
    });
    out.route.route.validity = validity.unwrap_or(candidate.route.route.validity);
    out.route.route.error = error;
    out.error = error;
    if error.is_some() {
        out.warning = None;
        out.support_data.warning = None;
    }
    out.support_data.error = error;
    out
}

/// The candidate with its root, and every joint but its socket's, moved to
/// the node of `key`.
fn moved_to_node(candidate: &TrunkBuildResult, spacing_mm: f64, key: &str) -> TrunkBuildResult {
    let (x, y) = grid_snapped_xy_from_key(key, spacing_mm);
    let (dx, dy) = (
        x - candidate.root.transform.pos.x,
        y - candidate.root.transform.pos.y,
    );
    if dx == 0.0 && dy == 0.0 {
        return candidate.clone();
    }
    let socket = candidate
        .trunk
        .contact_cone
        .as_ref()
        .and_then(|c| c.socket_joint_id.clone());
    let shift = |joint: &mut Option<Joint>| {
        if let Some(j) = joint
            && socket.as_deref() != Some(j.id.as_str())
        {
            j.pos.x += dx;
            j.pos.y += dy;
        }
    };
    let mut out = candidate.clone();
    for seg in &mut out.trunk.segments {
        shift(&mut seg.top_joint);
        shift(&mut seg.bottom_joint);
    }
    out.root.transform.pos.x = x;
    out.root.transform.pos.y = y;
    out.route.route.base_pos.x = x;
    out.route.route.base_pos.y = y;
    let route = &mut out.route.route;
    for j in route
        .joints
        .iter_mut()
        .chain(route.construction_joints.iter_mut())
    {
        j.x += dx;
        j.y += dy;
    }
    out.support_data.roots = Some(out.root.clone());
    out.support_data.segments = out.trunk.segments.clone();
    out
}

/// Where segment `index` of a trunk starts and ends: its own joints, else
/// the joint below it or the top of the root stack, and the joint above it
/// or the socket.
fn segment_ends(
    trunk: &Trunk,
    root: &Roots,
    index: usize,
    settings: &SupportSettings,
) -> Option<(Vec3, Vec3)> {
    let flare = if settings.base_flare.enabled {
        settings.base_flare.height_mm
    } else {
        0.0
    };
    let base = root.transform.pos;
    let stack_top = Vec3::new(
        base.x,
        base.y,
        base.z + settings.roots.disk_height_mm + flare,
    );
    let segment = trunk.segments.get(index)?;
    let start = match &segment.bottom_joint {
        Some(j) => j.pos,
        None if index == 0 => stack_top,
        None => trunk.segments[index - 1]
            .top_joint
            .as_ref()
            .map_or(stack_top, |j| j.pos),
    };
    let end = match (&segment.top_joint, &trunk.contact_cone) {
        (Some(j), _) => j.pos,
        (None, Some(cone)) => get_final_socket_position(cone),
        (None, None) => Vec3::new(start.x, start.y, start.z + 10.0),
    };
    Some((start, end))
}

/// Whether a member from `knot` to the contact, as a straight shaft to
/// where its socket would be, runs into the model.
fn member_blocked(
    knot: &Knot,
    h: &Hanging<'_, '_>,
    mesh: &dyn CollisionField,
    shaft_mm: f64,
) -> bool {
    let _perf = support_geom::perf::scope("grid:branch-collision");
    let reach = h.live.tip.length_mm;
    let socket = Vec3::new(
        h.tip_pos.x + h.tip_normal.x * reach,
        h.tip_pos.y + h.tip_normal.y * reach,
        h.tip_pos.z + h.tip_normal.z * reach,
    );
    is_shaft_blocked(knot.pos, socket, shaft_mm / 2.0 + 0.25, mesh)
}

/// The diameter of the host shaft at a knot (the knot is 0.1 mm wider).
fn host_width(knot: &Knot, settings: &SupportSettings) -> f64 {
    js_max(
        0.001,
        knot.diameter.unwrap_or(settings.shaft.diameter_mm + 0.1) - 0.1,
    )
}

fn on_knot(knot: &Knot) -> IndexMap<String, PlacementPrimitive> {
    IndexMap::from([(
        "parentKnotId".to_owned(),
        PlacementPrimitive::Knot(knot.clone()),
    )])
}

/// A leaf from `knot` to the contact, when the span is short and steep
/// enough for one.
fn leaf_from(
    key: &str,
    pillar: &Pillar<'_>,
    knot: &Knot,
    h: &mut Hanging<'_, '_>,
) -> Option<GridPlacementDecision> {
    let tip = h.tip_pos;
    let (dx, dy, dz) = (tip.x - knot.pos.x, tip.y - knot.pos.y, tip.z - knot.pos.z);
    let span2 = dx * dx + dy * dy + dz * dz;
    let span = span2.sqrt();
    if knot.pos.z > tip.z + 0.0001 || span > MAX_AUTO_LEAF_SPAN_MM {
        return None;
    }
    let from_up = if span2 < 0.000001 {
        0.0
    } else {
        rad_to_deg(js_acos(js_min(1.0, js_max(-1.0, dz / span))))
    };
    if from_up > h.settings.shaft.max_angle_deg.unwrap_or(80.0) {
        return None;
    }
    // The leaf is built without the model: the span is short.
    let built = build_leaf_data(
        &LeafBuildInput {
            tip_pos: tip,
            surface_normal: h.tip_normal,
            model_id: h.model_id,
            parent_knot: knot,
            host_diameter_mm: host_width(knot, h.settings),
            tip_contact_diameter_mm: None,
            mesh: None,
        },
        h.live,
        h.new_id,
    );
    let type_id = built
        .leaf
        .base
        .type_id
        .expect("a built member has its type");
    Some(GridPlacementDecision::Place {
        node_key: key.to_owned(),
        placed: placement_of_resolved(
            type_id,
            SupportEntityValue::Leaf(built.leaf),
            on_knot(knot),
            pillar.host(),
        ),
        support_data: Some(built.support_data),
    })
}

/// The contact hung on `pillar`: knots tried from the top of each segment
/// down (top segment first), each as a leaf when the span is short, else a
/// branch. The first member leaving the pillar at a trunk diagonal's lean or
/// steeper wins; else the steepest that leaves within its allowance.
/// `occupied`: the pillar stands on the contact's own node, so the member may
/// leave as flat as a socket's elbow.
fn hang_on(
    key: &str,
    pillar: &Pillar<'_>,
    h: &mut Hanging<'_, '_>,
    occupied: bool,
) -> Option<GridPlacementDecision> {
    let _perf = support_geom::perf::scope("grid:attach-search");
    let tip = h.tip_pos;
    let band_mm = h.settings.shaft.diameter_mm;
    let base_lean = 90.0 - h.min_angle_deg;
    let allowance = |span_mm: f64| {
        get_length_aware_max_angle_from_vertical_deg(span_mm, base_lean, Some(base_lean))
    };
    // The loosest lean a member can be allowed at all: a cheap first cut.
    let loosest = if occupied {
        SOCKET_ELBOW_MAX_ANGLE_FROM_VERTICAL_DEG
    } else {
        SHORT_SPAN_DETOUR_MAX_ANGLE_FROM_VERTICAL_DEG
    };
    let mut best: Option<GridPlacementDecision> = None;
    let mut best_lean = f64::INFINITY;
    for index in (0..pillar.trunk.segments.len()).rev() {
        let segment = &pillar.trunk.segments[index];
        let Some((start, end)) = segment_ends(pillar.trunk, pillar.root, index, h.settings) else {
            continue;
        };
        if start.z >= tip.z {
            continue;
        }
        let length = js_max(
            0.001,
            ((end.x - start.x) * (end.x - start.x)
                + (end.y - start.y) * (end.y - start.y)
                + (end.z - start.z) * (end.z - start.z))
                .sqrt(),
        );
        let dt = js_max(0.0005, h.step_mm / length);
        let mut t = 1.0;
        while t >= -1e-9 {
            // The segment's foot (t = 0) is always tried.
            let at = js_max(0.0, t);
            t -= dt;
            let pos = calculate_knot_position_on_segment_from_t(start, end, segment, at);
            if pos.z >= tip.z || member_departure_angle_from_vertical_deg(pos, tip) > loosest {
                continue;
            }
            let knot = Knot {
                id: (h.new_id)(),
                parent_shaft_id: segment.id.clone(),
                t: Some(at),
                pos,
                diameter: Some(segment.diameter + 0.1),
            };
            if let Some(mesh) = h.mesh
                && member_blocked(&knot, h, mesh, band_mm)
            {
                continue;
            }
            if let Some(leaf) = leaf_from(key, pillar, &knot, h) {
                let lean = member_departure_angle_from_vertical_deg(pos, tip);
                if lean > allowance(distance_3d(pos, tip)) {
                    continue;
                }
                if lean <= TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG {
                    return Some(leaf);
                }
                if lean < best_lean {
                    best = Some(leaf);
                    best_lean = lean;
                }
                continue;
            }
            // A branch, judged by its built shaft where it leaves the
            // pillar, not by the straight line to the contact. Its width
            // takes a share of the host's; when that is wider than the band
            // and does not clear, the band's width.
            let shared_mm = member_shaft_diameter_mm(
                band_mm,
                host_width(&knot, h.settings),
                h.settings.auto_support.member_host_shaft_ratio,
            );
            let build = |shaft_mm: f64, h: &mut Hanging<'_, '_>| {
                build_branch_data(
                    &BranchBuildInput {
                        tip_pos: tip,
                        tip_normal: h.tip_normal,
                        model_id: h.model_id,
                        parent_knot: &knot,
                        mesh: h.mesh,
                        shaft_diameter_mm: Some(shaft_mm),
                        tip_contact_diameter_mm: None,
                        roots_diameter_mm: None,
                        settings: None,
                        tip_profile: None,
                    },
                    h.live,
                    h.new_id,
                )
            };
            let mut built = build(shared_mm, h);
            if shared_mm > band_mm
                && let Some(mesh) = h.mesh
                && member_blocked(&knot, h, mesh, shared_mm)
            {
                built = build(band_mm, h);
            }
            let first_joint = built
                .branch
                .segments
                .first()
                .and_then(|s| s.top_joint.as_ref())
                .map(|j| j.pos);
            let lean = member_departure_angle_from_vertical_deg(pos, first_joint.unwrap_or(tip));
            if let Some(first) = first_joint
                && lean > allowance(distance_3d(pos, first))
            {
                continue;
            }
            let type_id = built
                .branch
                .base
                .type_id
                .expect("a built member has its type");
            let decision = GridPlacementDecision::Place {
                node_key: key.to_owned(),
                placed: placement_of_resolved(
                    type_id,
                    SupportEntityValue::Branch(built.branch),
                    on_knot(&knot),
                    pillar.host(),
                ),
                support_data: Some(built.support_data),
            };
            if lean <= TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG {
                return Some(decision);
            }
            if lean < best_lean {
                best = Some(decision);
                best_lean = lean;
            }
        }
    }
    best
}

/// The contact hung on a pillar of a node around the node of `key`.
///
/// Pillars set out by the grid: ring by ring outward, nearest nodes first,
/// as far as a 45° run from the tip could still reach (a node further on
/// may see past the model in between). Otherwise the eight neighbours, the
/// four sides first.
fn hang_nearby(
    key: &str,
    nodes: &IndexMap<String, Vec<Pillar<'_>>>,
    h: &mut Hanging<'_, '_>,
) -> Option<GridPlacementDecision> {
    let (gx, gy) = key_indices(key);
    let try_node = |dx: f64, dy: f64, h: &mut Hanging<'_, '_>| {
        let neighbour = node_key(gx + dx, gy + dy);
        let pillars = nodes.get(&neighbour).map(Vec::as_slice).unwrap_or(&[]);
        hang_on_any(pillars, &neighbour, h, false)
    };
    if !h.settings.grid.pillars {
        const AROUND: [(f64, f64); 8] = [
            (0.0, 1.0),
            (0.0, -1.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (1.0, 1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (-1.0, -1.0),
        ];
        return AROUND.iter().find_map(|&(dx, dy)| try_node(dx, dy, h));
    }
    let rings = ((h.tip_pos.z / h.settings.grid.spacing_mm).ceil() as i64).clamp(1, NEIGHBOR_RINGS);
    for ring in 1..=rings {
        let mut ring_nodes: Vec<(i64, i64)> = (-ring..=ring)
            .flat_map(|ix| (-ring..=ring).map(move |iy| (ix, iy)))
            .filter(|(ix, iy)| ix.abs() == ring || iy.abs() == ring)
            .collect();
        ring_nodes.sort_by_key(|(ix, iy)| ix * ix + iy * iy);
        if let Some(found) = ring_nodes
            .into_iter()
            .find_map(|(dx, dy)| try_node(dx as f64, dy as f64, h))
        {
            return Some(found);
        }
    }
    None
}

/// Whether a candidate pillar's shaft runs into the model, at its widest
/// segment's radius plus [`TRUNK_CLEARANCE_MM`].
fn pillar_blocked(
    candidate: &TrunkBuildResult,
    settings: &SupportSettings,
    mesh: &dyn CollisionField,
) -> bool {
    let _perf = support_geom::perf::scope("grid:trunk-collision");
    let trunk = &candidate.trunk;
    let widest = trunk
        .segments
        .iter()
        .fold(settings.shaft.diameter_mm, |m, s| js_max(m, s.diameter));
    let radius = widest / 2.0 + TRUNK_CLEARANCE_MM;
    (0..trunk.segments.len()).any(|i| {
        segment_ends(trunk, &candidate.root, i, settings)
            .is_some_and(|(start, end)| is_shaft_blocked(start, end, radius, mesh))
    })
}

fn refused(
    key: &str,
    reason: GridPlacementRejectReason,
    build: TrunkBuildResult,
) -> GridPlacementDecision {
    GridPlacementDecision::Reject {
        node_key: key.to_owned(),
        reason,
        trunk_build: Some(Box::new(build)),
        support_data: None,
    }
}

fn new_pillar(key: &str, type_id: SupportTypeId, build: TrunkBuildResult) -> GridPlacementDecision {
    let root = IndexMap::from([("rootId".to_owned(), PlacementPrimitive::Roots(build.root))]);
    GridPlacementDecision::Place {
        node_key: key.to_owned(),
        placed: placement_of_resolved(type_id, SupportEntityValue::Trunk(build.trunk), root, None),
        support_data: Some(build.support_data),
    }
}

/// The node nearest `reference`, or nearest the candidate's root.
fn node_near(
    candidate: &TrunkBuildResult,
    spacing_mm: f64,
    reference: Option<(f64, f64)>,
) -> String {
    let root = candidate.root.transform.pos;
    let (x, y) = reference.unwrap_or((root.x, root.y));
    grid_node_key_from_xy(x, y, spacing_mm)
}

/// What the grid makes of a contact and its candidate trunk.
///
/// A tip low enough for a stump is built by the stump's own rule. With the
/// grid off, the candidate stands where it was routed. Otherwise the
/// candidate is snapped to its node: on a free node it stands as a new
/// pillar if it clears the model, else hangs on a pillar nearby; on a node
/// a pillar holds (by key, or standing within half a step), it hangs on that
/// pillar or one nearby, and is refused when none can take it.
pub fn decide_grid_placement(
    args: &DecideGridPlacementArgs<'_>,
    live: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> GridPlacementDecision {
    let _perf = support_geom::perf::scope("grid:decide");
    let &DecideGridPlacementArgs {
        settings,
        snapshot,
        candidate,
        tip_pos,
        tip_normal,
        model_id,
        mesh,
    } = args;
    let spacing_mm = settings.grid.spacing_mm;

    let mut pillars: Vec<Pillar<'_>> = Vec::new();
    for kind in grid_host_types() {
        if !kind.owns_root || kind.id != SupportTypeId::Trunk {
            continue;
        }
        for (id, trunk) in &snapshot.collections.trunks {
            if trunk.base.model_id != model_id {
                continue;
            }
            if let Some(root) = snapshot.collections.roots.get(&trunk.root_id) {
                pillars.push(Pillar {
                    type_id: kind.id,
                    id,
                    trunk,
                    root,
                });
            }
        }
    }
    let nodes = pillars_by_node(&pillars, spacing_mm);

    let setting = |path: SupportPlacementSettingPath| match path {
        SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm => {
            Some(live.mesh_to_mesh.stick_vs_twig_cutoff_mm)
        }
    };
    let kind = select_type_for_placement(SupportPlacementMetric::TipHeight, tip_pos.z, &setting);
    if let Some(own) = kind
        && CONTACT_OVERRIDE_REGISTRATIONS.contains(&own)
    {
        let built = build_stump_contact_override(
            &ContactOverrideRequest {
                tip_pos,
                tip_normal,
                model_id,
                mesh,
            },
            live,
            new_id,
        );
        return match built.refusal {
            Some(refusal) => GridPlacementDecision::Reject {
                node_key: String::new(),
                reason: GridPlacementRejectReason::from_limitation(refusal),
                trunk_build: None,
                support_data: Some(built.support_data),
            },
            None => GridPlacementDecision::Place {
                node_key: String::new(),
                placed: built.placed,
                support_data: Some(built.support_data),
            },
        };
    }
    let Some(type_id) = kind else {
        panic!("no support type takes a tip {} mm high", tip_pos.z);
    };

    // Below the plate: snapping cannot save it.
    if candidate.error == Some(LimitationCode::OutOfBounds) {
        let key = if settings.grid.enabled {
            get_resolved_snapped_node_key(&candidate.route)
                .map(str::to_owned)
                .unwrap_or_else(|| node_near(candidate, spacing_mm, None))
        } else {
            GRID_OFF_KEY.to_owned()
        };
        return refused(
            &key,
            GridPlacementRejectReason::OutOfBounds,
            candidate.clone(),
        );
    }

    if !settings.grid.enabled {
        let build = with_snap(
            candidate,
            get_resolved_snapped_root_pos(&candidate.route, candidate.root.transform.pos),
            Some(GRID_OFF_KEY.to_owned()),
            get_resolved_snapped_validity(&candidate.route)
                .unwrap_or_else(|| get_default_snapped_validity(&candidate.route)),
            None,
            None,
        );
        return new_pillar(GRID_OFF_KEY, type_id, build);
    }

    let routed_key = get_resolved_snapped_node_key(&candidate.route).map(str::to_owned);
    let reference = candidate.route.route.unsnapped_bottom_pos;
    let key = routed_key
        .clone()
        .unwrap_or_else(|| node_near(candidate, spacing_mm, Some((reference.x, reference.y))));
    let on_node: Vec<Pillar<'_>> = nodes.get(&key).cloned().unwrap_or_default();
    let snapped =
        if has_resolved_snapped_root(&candidate.route) && Some(&key) == routed_key.as_ref() {
            candidate.clone()
        } else {
            moved_to_node(candidate, spacing_mm, &key)
        };
    let snapped_root = get_resolved_snapped_root_pos(&snapped.route, snapped.root.transform.pos);
    let mut h = Hanging {
        tip_pos,
        tip_normal,
        model_id,
        min_angle_deg: settings.grid.min_branch_angle_deg,
        settings,
        live,
        step_mm: settings.grid.attach_search_step_mm,
        mesh,
        new_id,
    };
    // Refused for want of a member a pillar could take.
    let no_member = |snapped: &TrunkBuildResult| {
        let build = with_snap(
            snapped,
            snapped_root,
            Some(key.clone()),
            get_default_snapped_validity(&snapped.route),
            None,
            Some(
                snapped
                    .route
                    .route
                    .error
                    .unwrap_or(LimitationCode::TooCloseToExisting),
            ),
        );
        refused(&key, GridPlacementRejectReason::NoValidAttachment, build)
    };

    if on_node.is_empty() {
        // Free by key, though a pillar between nodes may stand on it.
        let near = pillars_near(&pillars, &key, spacing_mm);
        if !near.is_empty() {
            return hang_on_any(&near, &key, &mut h, true)
                .or_else(|| hang_nearby(&key, &nodes, &mut h))
                .unwrap_or_else(|| no_member(&snapped));
        }
        if !mesh.is_some_and(|m| pillar_blocked(&snapped, settings, m)) {
            if candidate.route.route.grid_ignored == Some(true) {
                return new_pillar(OFF_GRID_KEY, type_id, candidate.clone());
            }
            let build = with_snap(
                &snapped,
                snapped_root,
                Some(key.clone()),
                get_resolved_snapped_validity(&snapped.route)
                    .unwrap_or_else(|| get_default_snapped_validity(&snapped.route)),
                None,
                None,
            );
            return new_pillar(&key, type_id, build);
        }
        if let Some(found) = hang_nearby(&key, &nodes, &mut h) {
            return found;
        }
        let build = with_snap(
            &snapped,
            snapped_root,
            Some(key.clone()),
            SnappedRouteValidity::HardInvalid,
            Some(TrunkRouteValidity::HardInvalid),
            Some(LimitationCode::CollisionWithModel),
        );
        return refused(&key, GridPlacementRejectReason::CollisionWithModel, build);
    }

    // A pillar holds the node: hang on it, never replace it.
    if on_node.iter().all(|p| p.trunk.segments.is_empty()) {
        let build = with_snap(
            &snapped,
            snapped_root,
            Some(key.clone()),
            SnappedRouteValidity::HardInvalid,
            Some(TrunkRouteValidity::HardInvalid),
            Some(LimitationCode::TooCloseToExisting),
        );
        return refused(&key, GridPlacementRejectReason::NoHostSegment, build);
    }
    hang_on_any(&on_node, &key, &mut h, true)
        .or_else(|| hang_nearby(&key, &nodes, &mut h))
        .unwrap_or_else(|| no_member(&snapped))
}

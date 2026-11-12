//! DragonFruit `src/supports/autoBracing/autoBrace.ts`, `buildAutoBracedSnapshot`:
//! brace a forest — shaft samples, a Voronoi partition into groups, model-wide
//! pairs (minimum spanning tree plus a second axis per support), kickstands for
//! tall trunks short of two axes, kickstand edges, the brace ladder (tiers or
//! zigzag chains) and the reconciliation that drops kickstands the ladder made
//! redundant.
//!
//! The store, the history push (`runAutoBracing`) and `clearBracesForModel` are
//! the GUI's. What DragonFruit reads globally is passed in: the live settings
//! (grid, kickstand sizing) and the meshes registered for clearance.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::brace_diameter::build_brace_profile;
use super::mesh_clearance::{BraceMeshes, line_passes_mesh_clearance, line_stays_off_mesh};
use super::patterns::{BracePair, apply_tier_pattern, run_zig_zag_chain};
use super::settings::{
    AutoBracingPattern, AutoBracingSettings, GridDiagonal, StackedBracing, auto_bracing_hard_rules,
    normalize_auto_bracing_settings,
};
use super::two_axis_detection::{
    axis_separation_deg, has_qualified_two_axis_bracing, normalize_axis_angle_rad,
};
use super::voronoi_partitioning::{
    Vec2, VoronoiPartitionSettings, VoronoiSupportNode, partition_supports_with_voronoi,
};
use crate::supports::auto_support::types::js_compare;
use crate::supports::constants::JOINT_DIAMETER_OFFSET_MM;
use crate::supports::placement_logic::grid::grid_math::snap_to_grid_index;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::knot::knot_utils::calculate_knot_position_on_segment_from_t;
use crate::supports::support_primitives::knot::segment_endpoints::{
    EndpointHosts, ShaftEntity, resolve_segment_endpoints,
};
use crate::supports::support_type_registry::{
    SupportTypeId, is_auto_braceable_shaft_type, is_lateral_stabiliser_type,
    lateral_stabiliser_types, parse_prefixed_segment_id,
};
use crate::supports::support_types::kickstand::kickstand_stabiliser::{
    BraceAxisEdge, GridSnap, StabiliserState, generate_required_kickstands,
};
use crate::supports::support_types::kickstand::types::KickstandBuildResult;
use crate::supports::types::{
    Brace, BraceDebugSection, Knot, Segment, SupportCollections, SupportEntity, SupportGeneratedBy,
    SupportState, Vec3,
};
use crate::three::math_utils::{clamp, js_atan2, js_max, js_min};
use crate::utils::collation::js_locale_compare;

const EPS: f64 = 0.000001;
const KICKSTAND_MAX_EDGES_PER_TRUNK: usize = 2;
const KICKSTAND_MAX_EDGES_PER_KICKSTAND: usize = 2;

/// Outcome of a bracing run, as a code the UI turns into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoBraceStatus {
    NoEligibleSupports,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoBraceResult {
    pub generated_brace_count: usize,
    pub removed_brace_count: usize,
    pub skipped_support_count: usize,
    pub changed: bool,
    pub status: AutoBraceStatus,
}

#[derive(Debug, Clone)]
struct SegmentSample {
    segment_id: String,
    segment: Segment,
    start: Vec3,
    end: Vec3,
    diameter_mm: f64,
}

#[derive(Debug, Clone)]
struct SupportSample {
    support_id: String,
    support_kind: SupportTypeId,
    model_id: String,
    segments: Vec<SegmentSample>,
    top_reference_z: f64,
    bottom_reference_z: f64,
    sort_anchor: Vec3,
    host_segment_id: Option<String>,
}

#[derive(Debug, Clone)]
struct AnchorPoint {
    model_id: String,
    segment_id: String,
    t: f64,
    pos: Vec3,
    host_diameter_mm: f64,
}

/// A pair of samples (indices into the run's sample arena).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Edge {
    a: usize,
    b: usize,
    h_dist: f64,
    angle_rad: f64,
}

fn sort_supports(a: &SupportSample, b: &SupportSample) -> std::cmp::Ordering {
    if a.model_id != b.model_id {
        return js_locale_compare(&a.model_id, &b.model_id);
    }
    if a.sort_anchor.x != b.sort_anchor.x {
        return js_compare(a.sort_anchor.x - b.sort_anchor.x);
    }
    if a.sort_anchor.y != b.sort_anchor.y {
        return js_compare(a.sort_anchor.y - b.sort_anchor.y);
    }
    js_locale_compare(&a.support_id, &b.support_id)
}

/// `createUniqueIdFactory`.
struct UniqueIds {
    prefix: &'static str,
    index: usize,
}

impl UniqueIds {
    fn next(&mut self, existing: &mut HashSet<String>) -> String {
        loop {
            let id = format!("{}-{}", self.prefix, self.index);
            self.index += 1;
            if existing.insert(id.clone()) {
                return id;
            }
        }
    }
}

fn collect_segment_extrema(segments: &[SegmentSample]) -> (f64, f64, Vec3) {
    let mut top: Option<Vec3> = None;
    let mut bottom: Option<Vec3> = None;
    for seg in segments {
        for p in [seg.start, seg.end] {
            if top.is_none_or(|t| p.z > t.z) {
                top = Some(p);
            }
            if bottom.is_none_or(|b| p.z < b.z) {
                bottom = Some(p);
            }
        }
    }
    (
        top.map_or(0.0, |p| p.z),
        bottom.map_or(0.0, |p| p.z),
        top.unwrap_or(Vec3::new(0.0, 0.0, 0.0)),
    )
}

/// Shaft samples for every auto-braceable type (trunk, branch, kickstand), in
/// registry order, sorted by model, plan position and id.
fn build_support_samples(c: &SupportCollections, store: &SupportCollections) -> Vec<SupportSample> {
    let mut supports = Vec::new();
    let mut push = |entity: ShaftEntity<'_>,
                    stamped: Option<SupportTypeId>,
                    kind: SupportTypeId,
                    model_id: &str,
                    segments: &[Segment],
                    hosts: EndpointHosts<'_>,
                    host_segment_id: Option<&str>| {
        // `resolveSegmentEndpoints` resolves the type first: an unstamped
        // entity the store does not hold (a kickstand this pass just built)
        // resolves to nothing and yields no segments.
        if stamped
            .or_else(|| store_type_of(store, entity.id()))
            .is_none()
        {
            return;
        }
        let samples: Vec<SegmentSample> = segments
            .iter()
            .enumerate()
            .filter_map(|(idx, seg)| {
                resolve_segment_endpoints(entity, seg, idx, hosts).map(|ep| SegmentSample {
                    segment_id: seg.id.clone(),
                    segment: seg.clone(),
                    start: ep.start,
                    end: ep.end,
                    diameter_mm: seg.diameter,
                })
            })
            .collect();
        if samples.is_empty() {
            return;
        }
        let (top, bottom, anchor) = collect_segment_extrema(&samples);
        supports.push(SupportSample {
            support_id: entity.id().to_owned(),
            support_kind: kind,
            model_id: model_id.to_owned(),
            segments: samples,
            top_reference_z: top,
            bottom_reference_z: bottom,
            sort_anchor: anchor,
            host_segment_id: host_segment_id.filter(|s| !s.is_empty()).map(str::to_owned),
        });
    };
    for t in c.trunks.values() {
        push(
            ShaftEntity::Trunk(t),
            t.base.type_id,
            SupportTypeId::Trunk,
            &t.base.model_id,
            &t.segments,
            EndpointHosts {
                root: (!t.root_id.is_empty())
                    .then(|| c.roots.get(&t.root_id))
                    .flatten(),
                host_knot: None,
            },
            None,
        );
    }
    for b in c.branches.values() {
        push(
            ShaftEntity::Branch(b),
            b.base.type_id,
            SupportTypeId::Branch,
            &b.base.model_id,
            &b.segments,
            EndpointHosts {
                root: None,
                host_knot: c.knots.get(&b.parent_knot_id),
            },
            None,
        );
    }
    for k in c.kickstands.values() {
        push(
            ShaftEntity::Kickstand(k),
            k.base.type_id,
            SupportTypeId::Kickstand,
            &k.base.model_id,
            &k.segments,
            EndpointHosts {
                root: (!k.root_id.is_empty())
                    .then(|| c.roots.get(&k.root_id))
                    .flatten(),
                host_knot: c.knots.get(&k.host_knot_id),
            },
            Some(&k.host_segment_id),
        );
    }
    supports.sort_by(sort_supports);
    supports
}

/// `getSupportTypeOf`, the store's resolver for an unstamped entity: the type
/// of the collection holding `id`, in registry order.
pub fn store_type_of(store: &SupportCollections, id: &str) -> Option<SupportTypeId> {
    let held = [
        (SupportTypeId::Trunk, store.trunks.contains_key(id)),
        (SupportTypeId::Branch, store.branches.contains_key(id)),
        (SupportTypeId::Leaf, store.leaves.contains_key(id)),
        (SupportTypeId::Twig, store.twigs.contains_key(id)),
        (SupportTypeId::Stick, store.sticks.contains_key(id)),
        (SupportTypeId::Brace, store.braces.contains_key(id)),
        (SupportTypeId::Stump, store.stumps.contains_key(id)),
        (SupportTypeId::Kickstand, store.kickstands.contains_key(id)),
    ];
    held.into_iter().find(|(_, h)| *h).map(|(t, _)| t)
}

fn resolve_anchor_at_z(support: &SupportSample, target_z: f64) -> Option<AnchorPoint> {
    let mut best: Option<(&SegmentSample, f64, Vec3, f64)> = None;
    for seg in &support.segments {
        let min_z = js_min(seg.start.z, seg.end.z);
        let max_z = js_max(seg.start.z, seg.end.z);
        if target_z < min_z - EPS || target_z > max_z + EPS {
            continue;
        }
        let dz = seg.end.z - seg.start.z;
        let t = if dz.abs() < EPS {
            0.0
        } else {
            (target_z - seg.start.z) / dz
        };
        let clamped_t = clamp(t, 0.0, 1.0);
        let pos =
            calculate_knot_position_on_segment_from_t(seg.start, seg.end, &seg.segment, clamped_t);
        let score = (pos.z - target_z).abs();
        if best.as_ref().is_none_or(|b| score < b.3 - EPS) {
            best = Some((seg, clamped_t, pos, score));
        }
    }
    let (seg, t, pos, _) = best?;
    Some(AnchorPoint {
        model_id: support.model_id.clone(),
        segment_id: seg.segment_id.clone(),
        t,
        pos,
        host_diameter_mm: seg.diameter_mm,
    })
}

fn reference_z_for_distance(a: &SupportSample, b: &SupportSample) -> f64 {
    let low = js_max(a.bottom_reference_z, b.bottom_reference_z);
    let high = js_min(a.top_reference_z, b.top_reference_z);
    if high > low + 0.001 {
        return (low + high) / 2.0;
    }
    js_min(a.top_reference_z, b.top_reference_z)
}

fn horizontal_distance_at_z(a: &SupportSample, b: &SupportSample, z: f64) -> Option<(f64, f64)> {
    let a_pos = resolve_anchor_at_z(a, z).map_or(a.sort_anchor, |p| p.pos);
    let b_pos = resolve_anchor_at_z(b, z).map_or(b.sort_anchor, |p| p.pos);
    let dx = b_pos.x - a_pos.x;
    let dy = b_pos.y - a_pos.y;
    let h_dist = (dx * dx + dy * dy).sqrt();
    if h_dist < 0.000001 {
        return None;
    }
    Some((h_dist, normalize_axis_angle_rad(js_atan2(dy, dx))))
}

fn support_bottom_anchor(support: &SupportSample) -> Vec3 {
    let mut bottom: Option<Vec3> = None;
    for seg in &support.segments {
        for p in [seg.start, seg.end] {
            if bottom.is_none_or(|b| p.z < b.z) {
                bottom = Some(p);
            }
        }
    }
    bottom.unwrap_or(support.sort_anchor)
}

fn grid_correlated_point(support: &SupportSample, grid: Option<GridSnap>) -> Vec2 {
    let base = support_bottom_anchor(support);
    match grid {
        Some(g) if g.enabled && !(g.spacing_mm <= 0.0) => Vec2 {
            x: snap_to_grid_index(base.x, g.spacing_mm) * g.spacing_mm,
            y: snap_to_grid_index(base.y, g.spacing_mm) * g.spacing_mm,
        },
        _ => Vec2 {
            x: base.x,
            y: base.y,
        },
    }
}

fn is_cardinal_delta(dx: f64, dy: f64, spacing_mm: f64) -> bool {
    let tol = js_max(0.1, spacing_mm * 0.05);
    dx.abs() <= tol || dy.abs() <= tol
}

fn min_pair_span_mm(settings: &AutoBracingSettings) -> f64 {
    js_max(
        auto_bracing_hard_rules::MIN_PAIR_SPAN_MM,
        settings.brace_diameter_mm,
    )
}

/// `buildGroupPairs`: candidate pairs within reach (cardinal only on a grid),
/// shortest first; the minimum spanning tree, then for each support short of
/// two axes the edge nearest a right angle that does not share a neighbour.
#[allow(clippy::needless_range_loop)]
fn build_group_pairs(
    arena: &[SupportSample],
    group: &[usize],
    max_len: f64,
    grid: Option<GridSnap>,
    min_span_mm: f64,
) -> Vec<Edge> {
    if group.len() < 2 {
        return Vec::new();
    }
    let max_run = max_len;
    let grid_cardinal_only = grid.is_some_and(|g| g.enabled);
    let tol = js_max(0.1, grid.map_or(1.0, |g| g.spacing_mm) * 0.05);
    let points: Vec<Vec2> = group
        .iter()
        .map(|&s| grid_correlated_point(&arena[s], grid))
        .collect();

    let mut edges: Vec<Edge> = Vec::new();
    for i in 0..group.len() {
        for j in i + 1..group.len() {
            let dx = points[j].x - points[i].x;
            let dy = points[j].y - points[i].y;
            if grid_cardinal_only && dx.abs() > tol && dy.abs() > tol {
                continue;
            }
            let h_dist = (dx * dx + dy * dy).sqrt();
            if h_dist < js_max(0.001, min_span_mm) || h_dist > max_run {
                continue;
            }
            edges.push(Edge {
                a: group[i],
                b: group[j],
                h_dist,
                angle_rad: normalize_axis_angle_rad(js_atan2(dy, dx)),
            });
        }
    }
    edges.sort_by(|x, y| js_compare(x.h_dist - y.h_dist));

    let id = |s: usize| arena[s].support_id.as_str();
    let mut result: Vec<Edge> = Vec::new();
    let mut adjacency: HashMap<&str, Vec<Edge>> =
        group.iter().map(|&s| (id(s), Vec::new())).collect();
    let mut edges_by_support: HashMap<&str, Vec<usize>> = HashMap::new();
    for (ei, e) in edges.iter().enumerate() {
        for s in [e.a, e.b] {
            edges_by_support.entry(id(s)).or_default().push(ei);
        }
    }
    let mut added: HashSet<usize> = HashSet::new();
    let link = |ei: usize,
                result: &mut Vec<Edge>,
                added: &mut HashSet<usize>,
                adjacency: &mut HashMap<&str, Vec<Edge>>| {
        let e = edges[ei];
        result.push(e);
        added.insert(ei);
        adjacency.get_mut(id(e.a)).expect("group member").push(e);
        adjacency.get_mut(id(e.b)).expect("group member").push(e);
    };

    // 1. Minimum spanning tree.
    let mut parent: HashMap<&str, &str> = group.iter().map(|&s| (id(s), id(s))).collect();
    fn find<'s>(parent: &HashMap<&'s str, &'s str>, x: &'s str) -> &'s str {
        let p = parent[x];
        if p == x { x } else { find(parent, p) }
    }
    for ei in 0..edges.len() {
        let e = edges[ei];
        let (ra, rb) = (find(&parent, id(e.a)), find(&parent, id(e.b)));
        if ra != rb {
            parent.insert(ra, rb);
            link(ei, &mut result, &mut added, &mut adjacency);
        }
    }

    // 2. Two-axis priority.
    let neighbours_of = |adjacency: &HashMap<&str, Vec<Edge>>, sid: &str| -> HashSet<String> {
        adjacency
            .get(sid)
            .map(|es| {
                es.iter()
                    .map(|e| if id(e.a) == sid { id(e.b) } else { id(e.a) }.to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    for &s in group {
        let sid = id(s);
        let axes: Vec<f64> = adjacency[sid].iter().map(|e| e.angle_rad).collect();
        if has_qualified_two_axis_bracing(&axes, auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG) {
            continue;
        }
        let mut best_candidate: Option<usize> = None;
        let mut best_score = -1.0;
        let own = neighbours_of(&adjacency, sid);
        for &ei in edges_by_support.get(sid).map_or(&[][..], Vec::as_slice) {
            if added.contains(&ei) {
                continue;
            }
            let e = edges[ei];
            let other = if id(e.a) == sid { id(e.b) } else { id(e.a) };
            // Skip a pair that already shares a braced neighbour.
            if neighbours_of(&adjacency, other)
                .iter()
                .any(|n| own.contains(n))
            {
                continue;
            }
            for &existing in &axes {
                let sep = axis_separation_deg(existing, e.angle_rad);
                if sep >= auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG {
                    let score = 90.0 - (90.0 - sep).abs();
                    if score > best_score {
                        best_score = score;
                        best_candidate = Some(ei);
                    }
                }
            }
        }
        if let Some(ei) = best_candidate {
            link(ei, &mut result, &mut added, &mut adjacency);
        }
    }
    result
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    // `[a, b].sort().join(':')`: code-unit order.
    if a.encode_utf16().cmp(b.encode_utf16()).is_le() {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

/// The ladder's brace writer (`place`), with what it accumulates.
struct Ladder<'a> {
    arena: &'a [SupportSample],
    settings: &'a AutoBracingSettings,
    grid: GridSnap,
    meshes: &'a BraceMeshes<'a>,
    max_run: f64,
    pair_distance_overrides: &'a HashSet<(String, String)>,
    brace_profile: crate::supports::types::BraceProfile,
    brace_ids: HashSet<String>,
    knot_ids: HashSet<String>,
    brace_id_factory: UniqueIds,
    knot_id_factory: UniqueIds,
    generated_braces: IndexMap<String, Brace>,
    generated_knots: IndexMap<String, Knot>,
    braced_axes_by_trunk_id: HashMap<String, Vec<f64>>,
}

impl Ladder<'_> {
    fn place(
        &mut self,
        low: usize,
        high: usize,
        section: BraceDebugSection,
        at_z: f64,
        min_rise_mm: f64,
    ) {
        let (low_s, high_s) = (&self.arena[low], &self.arena[high]);
        let ignore_max_distance = self
            .pair_distance_overrides
            .contains(&pair_key(&low_s.support_id, &high_s.support_id));
        let Some(low_anchor) = resolve_anchor_at_z(low_s, at_z) else {
            return;
        };
        let Some(same_tier) = resolve_anchor_at_z(high_s, at_z) else {
            return;
        };
        // Solve for a 45° link, never rising less than the floor.
        let mut dz_guess = js_max(
            min_rise_mm,
            ((same_tier.pos.x - low_anchor.pos.x).powi(2)
                + (same_tier.pos.y - low_anchor.pos.y).powi(2))
            .sqrt(),
        );
        for _ in 0..3 {
            let Some(high_anchor) = resolve_anchor_at_z(high_s, at_z + dz_guess) else {
                return;
            };
            let h_dist = ((high_anchor.pos.x - low_anchor.pos.x).powi(2)
                + (high_anchor.pos.y - low_anchor.pos.y).powi(2))
            .sqrt();
            let next_guess = js_max(min_rise_mm, h_dist);
            if (next_guess - dz_guess).abs() < 0.01 {
                dz_guess = next_guess;
                break;
            }
            dz_guess = next_guess;
            if dz_guess < EPS {
                return;
            }
        }
        if !ignore_max_distance && dz_guess > self.max_run + EPS {
            return;
        }
        if at_z + dz_guess >= low_s.top_reference_z - 0.1
            || at_z + dz_guess >= high_s.top_reference_z - 0.1
        {
            return;
        }
        let Some(high_anchor) = resolve_anchor_at_z(high_s, at_z + dz_guess) else {
            return;
        };
        let dx = high_anchor.pos.x - low_anchor.pos.x;
        let dy = high_anchor.pos.y - low_anchor.pos.y;
        if self.grid.enabled
            && !ignore_max_distance
            && !is_cardinal_delta(dx, dy, self.grid.spacing_mm)
        {
            return;
        }
        let horizontal_span = (dx * dx + dy * dy).sqrt();
        if !ignore_max_distance && horizontal_span > self.settings.max_brace_length_mm + EPS {
            return;
        }
        let mesh = self.meshes.get(&low_anchor.model_id);
        let diameter = self.settings.brace_diameter_mm;
        if !line_passes_mesh_clearance(low_anchor.pos, high_anchor.pos, mesh, diameter)
            || (self.settings.exact_clearance
                && !line_stays_off_mesh(low_anchor.pos, high_anchor.pos, mesh, diameter))
        {
            return;
        }
        self.commit(low, high, section, low_anchor, high_anchor);
    }

    /// Writes the brace from `low_anchor` to `high_anchor` and its knots.
    fn commit(
        &mut self,
        low: usize,
        high: usize,
        section: BraceDebugSection,
        low_anchor: AnchorPoint,
        high_anchor: AnchorPoint,
    ) {
        let (low_s, high_s) = (&self.arena[low], &self.arena[high]);
        // A bracing preset's brace keeps its end a diameter above where its
        // shaft starts, or the end would hang below the shaft (SoulCrafted).
        if self.settings.stacked.is_some() {
            let clear = self.settings.brace_diameter_mm;
            if low_anchor.pos.z < low_s.bottom_reference_z + clear
                || high_anchor.pos.z < high_s.bottom_reference_z + clear
            {
                return;
            }
        }
        let dx = high_anchor.pos.x - low_anchor.pos.x;
        let dy = high_anchor.pos.y - low_anchor.pos.y;
        let s_id = self.knot_id_factory.next(&mut self.knot_ids);
        let e_id = self.knot_id_factory.next(&mut self.knot_ids);
        let b_id = self.brace_id_factory.next(&mut self.brace_ids);
        // A bracing preset draws its braces at its own diameter (the
        // tessellation reads a brace's width off its knots), never thicker
        // than the thinner shaft it joins, or its end would stick out of it.
        let exact = self.settings.stacked.is_some().then(|| {
            self.settings
                .brace_diameter_mm
                .min(low_anchor.host_diameter_mm)
                .min(high_anchor.host_diameter_mm)
        });
        let knot = |id: &str, a: &AnchorPoint| Knot {
            id: id.to_owned(),
            parent_shaft_id: a.segment_id.clone(),
            t: Some(a.t),
            pos: a.pos,
            diameter: Some(exact.unwrap_or(a.host_diameter_mm) + JOINT_DIAMETER_OFFSET_MM),
            ..Knot::default()
        };
        self.generated_knots
            .insert(s_id.clone(), knot(&s_id, &low_anchor));
        self.generated_knots
            .insert(e_id.clone(), knot(&e_id, &high_anchor));
        self.generated_braces.insert(
            b_id.clone(),
            Brace {
                base: SupportEntity {
                    id: b_id,
                    model_id: low_anchor.model_id.clone(),
                    generated_by: Some(SupportGeneratedBy::AutoBracing),
                    ..SupportEntity::default()
                },
                start_knot_id: s_id,
                end_knot_id: e_id,
                placement_surface: None,
                curve: None,
                profile: self.brace_profile,
                debug_section: Some(section),
            },
        );
        // Only shaft-to-shaft braces count toward the two-axis contract.
        if is_auto_braceable_shaft_type(low_s.support_kind)
            && is_auto_braceable_shaft_type(high_s.support_kind)
        {
            let angle = normalize_axis_angle_rad(js_atan2(dy, dx));
            for tid in [&low_s.support_id, &high_s.support_id] {
                self.braced_axes_by_trunk_id
                    .entry(tid.clone())
                    .or_default()
                    .push(angle);
            }
        }
    }
}

/// A bracing preset's chains (SoulCrafted, #360): per pair, links from
/// `start_z` up, each rising by the pair's own span (never less than
/// `min_rise_mm`) and `gap_mm` above the last, in the bottom, middle or top
/// pattern by its place in the chain; then a ring (a cross) at every
/// reinforcement height, with its diagonal.
fn run_stacked_chain(
    pairs: &[BracePair<usize>],
    stack: &StackedBracing,
    start_z: f64,
    arena: &[SupportSample],
    ladder: &mut Ladder<'_>,
    min_rise_mm: f64,
    brace_diameter_mm: f64,
) {
    let width = |s: &SupportSample| s.segments.iter().map(|g| g.diameter_mm).fold(0.0, js_max);
    for (edge_index, edge) in pairs.iter().enumerate() {
        // Only shafts with a real gap between them: a brace between two
        // that touch is a stub inside them, and its ends hang in the open.
        let gap = edge.h_dist - (width(&arena[edge.a]) + width(&arena[edge.b])) / 2.0;
        if !(gap >= brace_diameter_mm) {
            continue;
        }
        let rise = js_max(edge.h_dist, min_rise_mm);
        let step = rise + stack.gap_mm;
        let top = js_min(arena[edge.a].top_reference_z, arena[edge.b].top_reference_z) - 0.1;
        if top <= start_z + rise {
            continue;
        }
        let links = (((top - start_z - rise) / step).floor() + 1.0).max(0.0) as u32;
        // Neighbouring pairs lean opposite ways, as the zigzag mirrors them.
        let (low, high) = if edge_index % 2 == 0 {
            (edge.a, edge.b)
        } else {
            (edge.b, edge.a)
        };
        for k in 0..links {
            let z = start_z + f64::from(k) * step;
            let section = if k == 0 {
                BraceDebugSection::Initial
            } else {
                BraceDebugSection::Repeating
            };
            ladder.place(low, high, section, z, rise);
            if stack.pattern_of(k, links) == AutoBracingPattern::CrossDiagonal {
                ladder.place(high, low, section, z, rise);
            }
        }
        for &h in &stack.reinforcement_heights_mm {
            // A cross at the height: a level brace would start in mid-air
            // (each layer of it lands on nothing). The ring's diagonal adds
            // one more link above it.
            ladder.place(low, high, BraceDebugSection::Repeating, h, rise);
            ladder.place(high, low, BraceDebugSection::Repeating, h, rise);
            let above = h + step;
            match stack.reinforcement_diagonal {
                GridDiagonal::None => {}
                GridDiagonal::Single => {
                    ladder.place(low, high, BraceDebugSection::Repeating, above, rise)
                }
                GridDiagonal::Cross => {
                    ladder.place(low, high, BraceDebugSection::Repeating, above, rise);
                    ladder.place(high, low, BraceDebugSection::Repeating, above, rise);
                }
            }
        }
    }
}

/// SoulCrafted (#3): braces one shaft added by hand to its neighbours, the
/// way a bracing run pairs and braces a shaft, and changes nothing else.
///
/// It pairs the shaft as [`build_group_pairs`] would: with the nearest
/// shaft of its model in reach, then with the one nearest a right angle to
/// that, for two axes. The pairs are laid by the run's own ladder (the
/// preset's chains, or its tiers and zigzags), each brace clear of the model,
/// within the settings' length, at their diameter. The braces and their knots
/// join `state`; the braces already there stay as they are. Returns how many
/// braces it added.
pub fn brace_new_shaft(
    state: &mut SupportState,
    shaft_id: &str,
    input_settings: &AutoBracingSettings,
    live: &SupportSettings,
    meshes: &BraceMeshes<'_>,
) -> usize {
    let settings = normalize_auto_bracing_settings(input_settings);
    let grid = GridSnap {
        enabled: live.grid.enabled && !live.grid.pillars,
        spacing_mm: live.grid.spacing_mm,
    };
    let c = &state.collections;
    let arena: Vec<SupportSample> = build_support_samples(c, c)
        .into_iter()
        .filter(|s| is_auto_braceable_shaft_type(s.support_kind))
        .collect();
    let Some(me) = arena.iter().position(|s| s.support_id == shaft_id) else {
        return 0;
    };
    let model_id = arena[me].model_id.clone();
    let min_span = js_max(0.001, min_pair_span_mm(&settings));
    let here = grid_correlated_point(&arena[me], Some(grid));
    let mut edges: Vec<Edge> = arena
        .iter()
        .enumerate()
        .filter(|(i, s)| *i != me && s.model_id == model_id)
        .filter_map(|(i, s)| {
            let there = grid_correlated_point(s, Some(grid));
            let (dx, dy) = (there.x - here.x, there.y - here.y);
            if grid.enabled && !is_cardinal_delta(dx, dy, grid.spacing_mm) {
                return None;
            }
            let h_dist = (dx * dx + dy * dy).sqrt();
            (h_dist >= min_span && h_dist <= settings.max_brace_length_mm).then(|| Edge {
                a: me,
                b: i,
                h_dist,
                angle_rad: normalize_axis_angle_rad(js_atan2(dy, dx)),
            })
        })
        .collect();
    edges.sort_by(|x, y| js_compare(x.h_dist - y.h_dist));
    let Some(&nearest) = edges.first() else {
        return 0;
    };
    // The second axis: the pair nearest a right angle to the first, as the
    // run's two-axis step picks it.
    let mut pairs = vec![nearest];
    let mut best: Option<(Edge, f64)> = None;
    for &e in &edges[1..] {
        let sep = axis_separation_deg(nearest.angle_rad, e.angle_rad);
        if sep < auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG {
            continue;
        }
        let score = 90.0 - (90.0 - sep).abs();
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((e, score));
        }
    }
    pairs.extend(best.map(|(e, _)| e));
    let max_z = arena
        .iter()
        .filter(|s| s.model_id == model_id)
        .fold(f64::NEG_INFINITY, |m, s| js_max(m, s.top_reference_z));
    let no_overrides = HashSet::new();
    let mut ladder = Ladder {
        arena: &arena,
        settings: &settings,
        grid,
        meshes,
        max_run: settings.max_brace_length_mm,
        pair_distance_overrides: &no_overrides,
        brace_profile: build_brace_profile(settings.brace_diameter_mm),
        brace_ids: c.braces.keys().cloned().collect(),
        knot_ids: c.knots.keys().cloned().collect(),
        brace_id_factory: UniqueIds {
            prefix: "auto-brace",
            index: 1,
        },
        knot_id_factory: UniqueIds {
            prefix: "auto-brace-knot",
            index: 1,
        },
        generated_braces: IndexMap::new(),
        generated_knots: IndexMap::new(),
        braced_axes_by_trunk_id: HashMap::new(),
    };
    lay_ladder(&mut ladder, &pairs, &settings, &arena, max_z);
    let Ladder {
        generated_braces,
        generated_knots,
        ..
    } = ladder;
    let added = generated_braces.len();
    state.collections.knots.extend(generated_knots);
    state.collections.braces.extend(generated_braces);
    added
}

/// Why a brace laid by hand between two shafts cannot go there (#3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandBraceRefusal {
    /// Both ends are on the same shaft.
    SameSupport,
    /// The shafts hold different models.
    OtherModel,
    /// One end is no shaft a brace can join (a leaf, a twig, nothing).
    NotAShaft,
    /// An end is off its shaft: above where its tip starts, or below where
    /// a brace may begin.
    OffShaft,
    /// The shafts are closer than a brace can span.
    TooClose,
    /// Longer than the bracing settings let a brace be.
    TooLong,
    /// At 45° from the first end, the second would be off its shaft: the
    /// engine lays no brace flatter than that.
    TooFlat,
    /// The brace would cross the model.
    HitsModel,
}

/// A brace laid by hand, with its knots: the first on the shaft it starts
/// from, the second on the other.
#[derive(Debug, Clone, PartialEq)]
pub struct HandBrace {
    pub brace: Brace,
    pub knots: [Knot; 2],
}

/// What [`hand_brace`] makes of two ends: the brace when it can go, else
/// why not, with the brace as it would be when both ends lie on their
/// shafts (to draw it red).
#[derive(Debug, Clone, PartialEq)]
pub struct HandBraceOutcome {
    pub brace: Option<HandBrace>,
    pub refusal: Option<HandBraceRefusal>,
}

/// SoulCrafted (#3): a brace laid by hand from shaft `from.0` at height
/// `from.1` to shaft `to.0` near height `to.1` (mm above the plate), by the
/// rules and sizes a bracing run lays one with: the ends where the run's
/// ladder anchors them (on the shaft's axis at that height), no flatter
/// than the run's 45° (a second end asked flatter is moved up or down its
/// shaft to 45°, the side it was asked), no longer than the settings'
/// length, clear of the model, at the settings' diameter. Ids come from
/// `new_id`; the brace is the person's (`generated_by` none), which a
/// bracing run's "remove existing" leaves alone. `state` is not changed.
pub fn hand_brace(
    state: &SupportState,
    from: (&str, f64),
    to: (&str, f64),
    input_settings: &AutoBracingSettings,
    live: &SupportSettings,
    meshes: &BraceMeshes<'_>,
    new_id: &mut dyn FnMut() -> String,
) -> HandBraceOutcome {
    let refused = |why| HandBraceOutcome {
        brace: None,
        refusal: Some(why),
    };
    if from.0 == to.0 {
        return refused(HandBraceRefusal::SameSupport);
    }
    let settings = normalize_auto_bracing_settings(input_settings);
    let c = &state.collections;
    let arena: Vec<SupportSample> = build_support_samples(c, c)
        .into_iter()
        .filter(|s| is_auto_braceable_shaft_type(s.support_kind))
        .collect();
    let find = |id: &str| arena.iter().position(|s| s.support_id == id);
    let (Some(a), Some(b)) = (find(from.0), find(to.0)) else {
        return refused(HandBraceRefusal::NotAShaft);
    };
    let (sa, sb) = (&arena[a], &arena[b]);
    if sa.model_id != sb.model_id {
        return refused(HandBraceRefusal::OtherModel);
    }
    // Where on a shaft an end may lie: below where its tip starts, as the
    // ladder keeps it, and for a bracing preset a diameter above where the
    // shaft starts, as its chains keep it.
    let floor = |s: &SupportSample| {
        s.bottom_reference_z
            + if settings.stacked.is_some() {
                settings.brace_diameter_mm
            } else {
                0.0
            }
    };
    let ceiling = |s: &SupportSample| s.top_reference_z - 0.1;
    let on = |s: &SupportSample, z: f64| z >= floor(s) - EPS && z <= ceiling(s) + EPS;
    let clamped = |s: &SupportSample, z: f64| js_max(floor(s), js_min(ceiling(s), z));
    let mut why: Option<HandBraceRefusal> = None;
    if !on(sa, from.1) || !on(sb, to.1) {
        why.get_or_insert(HandBraceRefusal::OffShaft);
    }
    let Some(low) = resolve_anchor_at_z(sa, clamped(sa, from.1)) else {
        return refused(HandBraceRefusal::OffShaft);
    };
    // Shafts farther apart than a brace may span: that says it first, as
    // no height on either makes it shorter.
    if let Some(there) = resolve_anchor_at_z(sb, clamped(sb, to.1)) {
        let h = ((there.pos.x - low.pos.x).powi(2) + (there.pos.y - low.pos.y).powi(2)).sqrt();
        if h > settings.max_brace_length_mm + EPS {
            why.get_or_insert(HandBraceRefusal::TooLong);
        }
    }
    // The second end: as asked, or at 45° when that is flatter, solved as
    // the ladder solves its links (the shafts may lean).
    let sign = if to.1 >= low.pos.z { 1.0 } else { -1.0 };
    let mut z = clamped(sb, to.1);
    let mut snapped = false;
    for _ in 0..4 {
        let Some(there) = resolve_anchor_at_z(sb, z) else {
            break;
        };
        let h = ((there.pos.x - low.pos.x).powi(2) + (there.pos.y - low.pos.y).powi(2)).sqrt();
        let rise = (to.1 - low.pos.z).abs();
        let want = if rise + EPS >= h {
            to.1
        } else {
            snapped = true;
            low.pos.z + sign * h
        };
        let next = clamped(sb, want);
        if !on(sb, want) {
            why.get_or_insert(if snapped {
                HandBraceRefusal::TooFlat
            } else {
                HandBraceRefusal::OffShaft
            });
        }
        if (next - z).abs() < 0.001 {
            z = next;
            break;
        }
        z = next;
    }
    let Some(high) = resolve_anchor_at_z(sb, z) else {
        return refused(why.unwrap_or(HandBraceRefusal::OffShaft));
    };
    let dx = high.pos.x - low.pos.x;
    let dy = high.pos.y - low.pos.y;
    let span = (dx * dx + dy * dy).sqrt();
    let rise = (high.pos.z - low.pos.z).abs();
    // Flatter than 45° once clamped to the shaft: the 45° end is off it.
    if rise + 0.01 < span {
        why.get_or_insert(HandBraceRefusal::TooFlat);
    }
    if span < js_max(0.001, min_pair_span_mm(&settings)) {
        why.get_or_insert(HandBraceRefusal::TooClose);
    }
    if span > settings.max_brace_length_mm + EPS || rise > settings.max_brace_length_mm + EPS {
        why.get_or_insert(HandBraceRefusal::TooLong);
    }
    let mesh = meshes.get(&low.model_id);
    if !line_passes_mesh_clearance(low.pos, high.pos, mesh, settings.brace_diameter_mm)
        || (settings.exact_clearance
            && !line_stays_off_mesh(low.pos, high.pos, mesh, settings.brace_diameter_mm))
    {
        why.get_or_insert(HandBraceRefusal::HitsModel);
    }
    // A bracing preset draws its braces at its own diameter, never thicker
    // than the thinner shaft they join, as the ladder's writer does.
    let exact = settings.stacked.is_some().then(|| {
        settings
            .brace_diameter_mm
            .min(low.host_diameter_mm)
            .min(high.host_diameter_mm)
    });
    // The ladder's own writer, without the preset's height rule it would
    // refuse an end by in silence: the ends were kept on their shafts above.
    let plain = AutoBracingSettings {
        stacked: None,
        ..settings.clone()
    };
    let no_overrides = HashSet::new();
    let mut ladder = Ladder {
        arena: &arena,
        settings: &plain,
        grid: GridSnap {
            enabled: live.grid.enabled && !live.grid.pillars,
            spacing_mm: live.grid.spacing_mm,
        },
        meshes,
        max_run: settings.max_brace_length_mm,
        pair_distance_overrides: &no_overrides,
        brace_profile: build_brace_profile(settings.brace_diameter_mm),
        brace_ids: c.braces.keys().cloned().collect(),
        knot_ids: c.knots.keys().cloned().collect(),
        brace_id_factory: UniqueIds {
            prefix: "auto-brace",
            index: 1,
        },
        knot_id_factory: UniqueIds {
            prefix: "auto-brace-knot",
            index: 1,
        },
        generated_braces: IndexMap::new(),
        generated_knots: IndexMap::new(),
        braced_axes_by_trunk_id: HashMap::new(),
    };
    ladder.commit(a, b, BraceDebugSection::Initial, low, high);
    let Some((_, mut brace)) = ladder.generated_braces.pop() else {
        return refused(why.unwrap_or(HandBraceRefusal::OffShaft));
    };
    let mut knots = ladder.generated_knots.into_values();
    let (Some(mut start), Some(mut end)) = (knots.next(), knots.next()) else {
        return refused(why.unwrap_or(HandBraceRefusal::OffShaft));
    };
    if let Some(exact) = exact {
        start.diameter = Some(exact + JOINT_DIAMETER_OFFSET_MM);
        end.diameter = Some(exact + JOINT_DIAMETER_OFFSET_MM);
    }
    start.id = new_id();
    end.id = new_id();
    brace.base.id = new_id();
    brace.base.generated_by = None;
    brace.start_knot_id = start.id.clone();
    brace.end_knot_id = end.id.clone();
    HandBraceOutcome {
        brace: Some(HandBrace {
            brace,
            knots: [start, end],
        }),
        refusal: why,
    }
}

/// The brace ladder of one model's pairs, `max_z` its tallest shaft top:
/// a bracing preset's chains, else the zigzag chains and the pattern tiers.
fn lay_ladder(
    ladder: &mut Ladder<'_>,
    pairs: &[Edge],
    settings: &AutoBracingSettings,
    arena: &[SupportSample],
    max_z: f64,
) {
    let mut tiers = vec![settings.initial_distance_mm];
    let mut curr = settings.initial_distance_mm + settings.pattern_interval_mm;
    while curr <= max_z {
        tiers.push(curr);
        curr += settings.pattern_interval_mm;
    }
    let brace_pairs: Vec<BracePair<usize>> = pairs
        .iter()
        .map(|e| BracePair {
            a: e.a,
            b: e.b,
            h_dist: e.h_dist,
        })
        .collect();
    let min_rise = auto_bracing_hard_rules::MIN_ZIG_ZAG_RISE_MM;
    if let Some(stack) = &settings.stacked {
        run_stacked_chain(
            &brace_pairs,
            stack,
            settings.initial_distance_mm,
            arena,
            ladder,
            min_rise,
            settings.brace_diameter_mm,
        );
        return;
    }
    if settings.initial_pattern == AutoBracingPattern::ZigZag {
        run_zig_zag_chain(
            &brace_pairs,
            settings.initial_distance_mm,
            max_z,
            BraceDebugSection::Initial,
            &mut |l, h, s, z, r| ladder.place(l, h, s, z, r),
            min_rise,
        );
    } else if settings.repeating_pattern == AutoBracingPattern::ZigZag {
        run_zig_zag_chain(
            &brace_pairs,
            settings.initial_distance_mm + settings.pattern_interval_mm,
            max_z,
            BraceDebugSection::Repeating,
            &mut |l, h, s, z, r| ladder.place(l, h, s, z, r),
            min_rise,
        );
    }
    for (tier_index, &anchor_z) in tiers.iter().enumerate() {
        let is_initial = tier_index == 0;
        let pattern = if is_initial {
            settings.initial_pattern
        } else {
            settings.repeating_pattern
        };
        if pattern == AutoBracingPattern::ZigZag {
            continue;
        }
        apply_tier_pattern(
            &brace_pairs,
            pattern,
            if is_initial {
                BraceDebugSection::Initial
            } else {
                BraceDebugSection::Repeating
            },
            &mut |l, h, s| ladder.place(l, h, s, anchor_z, 0.0),
        );
    }
}

/// How far, in mm, a shaft may stand above its highest brace before it is
/// braced higher up (#464).
const UNBRACED_TOP_MM: f64 = 10.0;

/// SoulCrafted (#464): braces every trunk and branch among the first `shafts`
/// of `arena` that stands more than [`UNBRACED_TOP_MM`] above its highest
/// brace (or above where it starts, when none holds it) to the shafts nearest
/// it up there: shafts that rise beside it more than [`UNBRACED_TOP_MM`]
/// above that brace. A pair is measured at the middle of the heights both
/// span there, not where they stand, so a trunk that bends away from its
/// neighbours finds the ones it runs beside. Each shaft takes the
/// nearest, and a second on another axis, as the pairs below do; the links
/// climb from that brace by the run's chain, without its reinforcement rings,
/// each one held to the ladder's length and clearance. `held` are the knots
/// of the braces kept from before the run.
fn brace_tall_shafts<'k>(
    ladder: &mut Ladder<'_>,
    arena: &[SupportSample],
    shafts: usize,
    settings: &AutoBracingSettings,
    held: impl Iterator<Item = &'k Knot>,
) {
    let is_shaft = |i: usize| {
        matches!(
            arena[i].support_kind,
            SupportTypeId::Trunk | SupportTypeId::Branch
        )
    };
    let owner: HashMap<&str, usize> = (0..shafts)
        .flat_map(|i| {
            arena[i]
                .segments
                .iter()
                .map(move |g| (g.segment_id.as_str(), i))
        })
        .collect();
    let mut braced = vec![f64::NEG_INFINITY; shafts];
    let note = |knot: &Knot, braced: &mut Vec<f64>| {
        if let Some(&i) = owner.get(knot.parent_shaft_id.as_str()) {
            braced[i] = js_max(braced[i], knot.pos.z);
        }
    };
    for knot in held {
        note(knot, &mut braced);
    }
    // Where each shaft reaches in the plane, to skip pairs far apart.
    let reach: Vec<[f64; 4]> = (0..shafts)
        .map(|i| {
            arena[i].segments.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |r, g| {
                    [
                        r[0].min(g.start.x).min(g.end.x),
                        r[1].min(g.start.y).min(g.end.y),
                        r[2].max(g.start.x).max(g.end.x),
                        r[3].max(g.start.y).max(g.end.y),
                    ]
                },
            )
        })
        .collect();
    let apart = |a: usize, b: usize| {
        let (p, q) = (reach[a], reach[b]);
        let dx = js_max(0.0, js_max(q[0] - p[2], p[0] - q[2]));
        let dy = js_max(0.0, js_max(q[1] - p[3], p[1] - q[3]));
        (dx * dx + dy * dy).sqrt()
    };
    let stack = StackedBracing {
        reinforcement_heights_mm: Vec::new(),
        ..settings.stacked.clone().unwrap_or_default()
    };
    let min_rise = auto_bracing_hard_rules::MIN_ZIG_ZAG_RISE_MM;
    let min_span = min_pair_span_mm(settings);
    let max_len = settings.max_brace_length_mm;
    let mut seen = 0;
    for s in 0..shafts {
        // The braces the shafts before this one were given count too.
        for knot in ladder.generated_knots.values().skip(seen) {
            note(knot, &mut braced);
        }
        seen = ladder.generated_knots.len();
        if !is_shaft(s) {
            continue;
        }
        let sample = &arena[s];
        let from = if braced[s].is_finite() {
            braced[s]
        } else {
            sample.bottom_reference_z
        };
        let from = js_max(from, settings.initial_distance_mm);
        if sample.top_reference_z - from <= UNBRACED_TOP_MM {
            continue;
        }
        let mut near: Vec<(Edge, f64)> = Vec::new();
        for (t, other) in arena.iter().enumerate().take(shafts) {
            if t == s || !is_shaft(t) || other.model_id != sample.model_id || apart(s, t) > max_len
            {
                continue;
            }
            let low = js_max(from, other.bottom_reference_z);
            let high = js_min(sample.top_reference_z, other.top_reference_z) - 0.1;
            // Only a shaft that rises beside it through the part left bare.
            if high - low <= UNBRACED_TOP_MM {
                continue;
            }
            let Some((h_dist, angle_rad)) =
                horizontal_distance_at_z(sample, other, (low + high) / 2.0)
            else {
                continue;
            };
            if h_dist < min_span || h_dist > max_len || high <= low + js_max(h_dist, min_rise) {
                continue;
            }
            near.push((
                Edge {
                    a: s,
                    b: t,
                    h_dist,
                    angle_rad,
                },
                low,
            ));
        }
        near.sort_by(|x, y| js_compare(x.0.h_dist - y.0.h_dist));
        let Some(&first) = near.first() else {
            continue;
        };
        // The second: the one nearest a right angle to the first.
        let off_square =
            |e: &Edge| (90.0 - axis_separation_deg(first.0.angle_rad, e.angle_rad)).abs();
        let second = near
            .iter()
            .filter(|(e, _)| {
                axis_separation_deg(first.0.angle_rad, e.angle_rad)
                    >= auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG
            })
            .min_by(|(x, _), (y, _)| js_compare(off_square(x) - off_square(y)))
            .copied();
        for (edge, low) in std::iter::once(first).chain(second) {
            run_stacked_chain(
                &[BracePair {
                    a: edge.a,
                    b: edge.b,
                    h_dist: edge.h_dist,
                }],
                &stack,
                low,
                arena,
                ladder,
                min_rise,
                settings.brace_diameter_mm,
            );
        }
    }
}

/// `buildAutoBracedSnapshot`. `live` is `getSettings()` (grid, kickstand
/// sizing); `store` is the committed store, which resolves the type of an
/// unstamped entity; ids are drawn by the kickstand builds.
#[allow(clippy::needless_range_loop)]
pub fn build_auto_braced_snapshot(
    snapshot: &SupportState,
    input_settings: &AutoBracingSettings,
    live: &SupportSettings,
    meshes: &BraceMeshes<'_>,
    store: &SupportState,
    new_id: &mut dyn FnMut() -> String,
) -> (SupportState, AutoBraceResult) {
    let c = &snapshot.collections;
    let settings = normalize_auto_bracing_settings(input_settings);
    let grid = GridSnap {
        enabled: live.grid.enabled && !live.grid.pillars,
        spacing_mm: live.grid.spacing_mm,
    };
    let max_run = settings.max_brace_length_mm;

    let mut arena: Vec<SupportSample> = build_support_samples(c, &store.collections)
        .into_iter()
        .filter(|s| is_auto_braceable_shaft_type(s.support_kind))
        .collect();
    let trunk_count = arena.len();
    if trunk_count < auto_bracing_hard_rules::MIN_GROUP_SIZE {
        return (
            snapshot.clone(),
            AutoBraceResult {
                generated_brace_count: 0,
                removed_brace_count: 0,
                skipped_support_count: trunk_count,
                changed: false,
                status: AutoBraceStatus::NoEligibleSupports,
            },
        );
    }

    // Keep every stabiliser this run did not generate, with what it claims.
    let mut stabiliser_state = StabiliserState {
        kickstands: c.kickstands.clone(),
        roots: c.roots.clone(),
        knots: c.knots.clone(),
    };
    let mut selected_stabiliser_cleared = false;
    {
        let mut next = StabiliserState::default();
        let mut removed = 0;
        for (id, k) in &stabiliser_state.kickstands {
            if k.base.generated_by == Some(SupportGeneratedBy::AutoBracing) {
                removed += 1;
                continue;
            }
            next.kickstands.insert(id.clone(), k.clone());
            if let Some(root) = stabiliser_state.roots.get(&k.root_id) {
                next.roots.insert(root.base.id.clone(), root.clone());
            }
            if let Some(knot) = stabiliser_state.knots.get(&k.host_knot_id) {
                next.knots.insert(knot.id.clone(), knot.clone());
            }
        }
        if removed > 0 {
            if let Some(sel) = &snapshot.selected_id
                && c.kickstands.contains_key(sel)
                && !next.kickstands.contains_key(sel)
            {
                selected_stabiliser_cleared = true;
            }
            stabiliser_state = next;
        }
    }

    let trunk_index_by_id: HashMap<String, usize> = (0..trunk_count)
        .map(|i| (arena[i].support_id.clone(), i))
        .collect();
    let trunk_group_ids = partition_supports_with_voronoi(
        &(0..trunk_count)
            .map(|i| VoronoiSupportNode {
                support_id: arena[i].support_id.clone(),
                model_id: arena[i].model_id.clone(),
                point: grid_correlated_point(&arena[i], Some(grid)),
            })
            .collect::<Vec<_>>(),
        &VoronoiPartitionSettings {
            seed_spacing_mm: settings.seed_spacing_mm,
            seed_jitter_mm: settings.seed_jitter_mm,
            max_neighbor_distance_mm: max_run,
        },
    );

    // Preliminary model-wide pairing, for the kickstand decisions.
    let mut trunks_by_model: IndexMap<String, Vec<usize>> = IndexMap::new();
    for i in 0..trunk_count {
        trunks_by_model
            .entry(arena[i].model_id.clone())
            .or_default()
            .push(i);
    }
    let mut existing_trunk_edges: Vec<BraceAxisEdge> = Vec::new();
    for model_trunks in trunks_by_model.values() {
        for pair in build_group_pairs(
            &arena,
            model_trunks,
            settings.max_brace_length_mm,
            Some(grid),
            min_pair_span_mm(&settings),
        ) {
            existing_trunk_edges.push(BraceAxisEdge {
                a: arena[pair.a].support_id.clone(),
                b: arena[pair.b].support_id.clone(),
                angle_rad: pair.angle_rad,
            });
        }
    }

    // Generative phase: kickstands for tall trunks short of two axes.
    let mut generated_stabilisers: Vec<KickstandBuildResult> = Vec::new();
    for type_id in lateral_stabiliser_types() {
        if type_id == SupportTypeId::Kickstand {
            generated_stabilisers.extend(generate_required_kickstands(
                snapshot,
                &stabiliser_state,
                &settings,
                &existing_trunk_edges,
                grid,
                meshes,
                live,
                store,
                new_id,
            ));
        }
    }
    let mut generated_stabiliser_ids: HashSet<String> = HashSet::new();
    if !generated_stabilisers.is_empty() {
        for build in &mut generated_stabilisers {
            if build.kickstand.host_segment_id.is_empty() {
                build.kickstand.host_segment_id = build.host_knot.parent_shaft_id.clone();
            }
            build.kickstand.base.generated_by = Some(SupportGeneratedBy::AutoBracing);
            generated_stabiliser_ids.insert(build.kickstand.base.id.clone());
            stabiliser_state
                .kickstands
                .insert(build.kickstand.base.id.clone(), build.kickstand.clone());
            stabiliser_state
                .roots
                .insert(build.root.base.id.clone(), build.root.clone());
            stabiliser_state
                .knots
                .insert(build.host_knot.id.clone(), build.host_knot.clone());
        }
    }

    // Stabiliser samples join the arena after the shafts.
    let stabiliser_collections = SupportCollections {
        kickstands: stabiliser_state.kickstands.clone(),
        roots: stabiliser_state.roots.clone(),
        knots: stabiliser_state.knots.clone(),
        ..SupportCollections::default()
    };
    let stabiliser_start = arena.len();
    arena.extend(build_support_samples(
        &stabiliser_collections,
        &store.collections,
    ));
    let arena = arena;

    let mut segment_owner_trunk_id: HashMap<&str, &str> = HashMap::new();
    for t in c.trunks.values() {
        for seg in &t.segments {
            segment_owner_trunk_id.insert(seg.id.as_str(), t.base.id.as_str());
        }
    }

    let find_nearest_trunk = |sb: &SupportSample| -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_dist = f64::INFINITY;
        for i in 0..trunk_count {
            let trunk = &arena[i];
            if trunk.model_id != sb.model_id {
                continue;
            }
            let z = reference_z_for_distance(trunk, sb);
            let Some((h_dist, _)) = horizontal_distance_at_z(trunk, sb, z) else {
                continue;
            };
            if h_dist < best_dist {
                best_dist = h_dist;
                best = Some(i);
            }
        }
        best.filter(|_| !(best_dist > max_run + EPS))
    };
    let mut assigned_host_by_stabiliser: HashMap<usize, String> = HashMap::new();
    let mut stabilisers_by_host_id: HashMap<String, Vec<usize>> = HashMap::new();
    for k in stabiliser_start..arena.len() {
        let sb = &arena[k];
        let host_trunk_id = sb
            .host_segment_id
            .as_deref()
            .and_then(|s| segment_owner_trunk_id.get(s).map(|t| (*t).to_owned()));
        let Some(assigned) =
            host_trunk_id.or_else(|| find_nearest_trunk(sb).map(|i| arena[i].support_id.clone()))
        else {
            continue;
        };
        assigned_host_by_stabiliser.insert(k, assigned.clone());
        stabilisers_by_host_id.entry(assigned).or_default().push(k);
    }

    let mut grouped_supports: Vec<Vec<usize>> = Vec::new();
    for group_ids in &trunk_group_ids {
        let g: Vec<usize> = group_ids
            .iter()
            .filter_map(|id| trunk_index_by_id.get(id).copied())
            .collect();
        let mut members = g.clone();
        for &t in &g {
            if let Some(ks) = stabilisers_by_host_id.get(&arena[t].support_id) {
                members.extend(ks.iter().copied());
            }
        }
        grouped_supports.push(members);
    }
    let grouped_ids: HashSet<&str> = grouped_supports
        .iter()
        .flatten()
        .filter(|&&s| is_auto_braceable_shaft_type(arena[s].support_kind))
        .map(|&s| arena[s].support_id.as_str())
        .collect();

    // Braces this tool did not generate stay.
    let kept_braces: IndexMap<String, Brace> = c
        .braces
        .iter()
        .filter(|(_, b)| {
            !(b.base.generated_by == Some(SupportGeneratedBy::AutoBracing)
                && settings.remove_existing_bracing)
        })
        .map(|(id, b)| (id.clone(), b.clone()))
        .collect();
    let brace_knot_ids: HashSet<&str> = c
        .braces
        .values()
        .flat_map(|b| [b.start_knot_id.as_str(), b.end_knot_id.as_str()])
        .collect();
    // Every knot something still hangs from, by declared knot edges; braces
    // read from the kept set so removed ones do not hold their knots.
    let mut preserved: HashSet<&str> = HashSet::new();
    preserved.extend(c.branches.values().map(|e| e.parent_knot_id.as_str()));
    preserved.extend(c.leaves.values().map(|e| e.parent_knot_id.as_str()));
    preserved.extend(
        kept_braces
            .values()
            .flat_map(|b| [b.start_knot_id.as_str(), b.end_knot_id.as_str()]),
    );
    preserved.extend(c.kickstands.values().map(|e| e.host_knot_id.as_str()));
    let next_knots: IndexMap<String, Knot> = c
        .knots
        .iter()
        .filter(|(id, _)| !brace_knot_ids.contains(id.as_str()) || preserved.contains(id.as_str()))
        .map(|(id, k)| (id.clone(), k.clone()))
        .collect();

    let selected_brace_id = snapshot.selected_id.as_deref().map(|sel| {
        parse_prefixed_segment_id(sel)
            .map_or(sel, |(_, entity)| entity)
            .to_owned()
    });
    let next_selected = match &selected_brace_id {
        Some(id) if c.braces.contains_key(id) && !kept_braces.contains_key(id) => None,
        _ => snapshot.selected_id.clone(),
    };

    let mut ladder_pairs: IndexMap<String, Vec<Edge>> = IndexMap::new();
    let mut pair_distance_overrides: HashSet<(String, String)> = HashSet::new();

    for group_members in &grouped_supports {
        let group_trunks: Vec<usize> = group_members
            .iter()
            .copied()
            .filter(|&s| is_auto_braceable_shaft_type(arena[s].support_kind))
            .collect();
        let model_id = group_trunks.first().map(|&s| arena[s].model_id.clone());
        let mut scratch: Vec<Edge>;
        let pairs: &mut Vec<Edge> = match &model_id {
            Some(m) => ladder_pairs.entry(m.clone()).or_insert_with(|| {
                let model_trunks: Vec<usize> = (0..trunk_count)
                    .filter(|&i| &arena[i].model_id == m)
                    .collect();
                build_group_pairs(
                    &arena,
                    &model_trunks,
                    settings.max_brace_length_mm,
                    Some(grid),
                    min_pair_span_mm(&settings),
                )
            }),
            None => {
                scratch = Vec::new();
                &mut scratch
            }
        };
        let extra: Vec<usize> = group_members
            .iter()
            .copied()
            .filter(|&s| is_lateral_stabiliser_type(arena[s].support_kind))
            .collect();
        if extra.is_empty() || group_trunks.is_empty() {
            continue;
        }

        let distance = |a: &SupportSample, b: &SupportSample| -> Option<(f64, f64)> {
            if grid.enabled {
                let ab = grid_correlated_point(a, Some(grid));
                let bb = grid_correlated_point(b, Some(grid));
                let (dx, dy) = (bb.x - ab.x, bb.y - ab.y);
                if !is_cardinal_delta(dx, dy, grid.spacing_mm) {
                    return None;
                }
                let h_dist = (dx * dx + dy * dy).sqrt();
                if h_dist < 0.000001 {
                    return None;
                }
                Some((h_dist, normalize_axis_angle_rad(js_atan2(dy, dx))))
            } else {
                horizontal_distance_at_z(a, b, reference_z_for_distance(a, b))
            }
        };
        let mut candidate_edges: Vec<Edge> = Vec::new();
        for &sb in &extra {
            let ignore = generated_stabiliser_ids.contains(&arena[sb].support_id);
            for &trunk in &group_trunks {
                let Some((h_dist, angle_rad)) = distance(&arena[trunk], &arena[sb]) else {
                    continue;
                };
                if h_dist > max_run + EPS && !ignore {
                    continue;
                }
                candidate_edges.push(Edge {
                    a: trunk,
                    b: sb,
                    h_dist,
                    angle_rad,
                });
            }
        }
        for i in 0..extra.len() {
            for j in i + 1..extra.len() {
                let (sb1, sb2) = (extra[i], extra[j]);
                let ignore = generated_stabiliser_ids.contains(&arena[sb1].support_id)
                    || generated_stabiliser_ids.contains(&arena[sb2].support_id);
                let Some((h_dist, angle_rad)) = distance(&arena[sb1], &arena[sb2]) else {
                    continue;
                };
                if h_dist > max_run + EPS && !ignore {
                    continue;
                }
                candidate_edges.push(Edge {
                    a: sb1,
                    b: sb2,
                    h_dist,
                    angle_rad,
                });
            }
        }

        let edge_id = |e: &Edge| pair_key(&arena[e.a].support_id, &arena[e.b].support_id);
        let mut existing_edge_ids: HashSet<(String, String)> = pairs.iter().map(edge_id).collect();
        let mut trunk_edge_count: HashMap<usize, usize> = HashMap::new();
        let mut stabiliser_edge_count: HashMap<usize, usize> = HashMap::new();
        let can_take = |tc: &HashMap<usize, usize>, sc: &HashMap<usize, usize>, e: &Edge| {
            tc.get(&e.a).copied().unwrap_or(0) < KICKSTAND_MAX_EDGES_PER_TRUNK
                && sc.get(&e.b).copied().unwrap_or(0) < KICKSTAND_MAX_EDGES_PER_KICKSTAND
        };
        let mut add_edge = |e: Edge,
                            pairs: &mut Vec<Edge>,
                            existing: &mut HashSet<(String, String)>,
                            tc: &mut HashMap<usize, usize>,
                            sc: &mut HashMap<usize, usize>| {
            pairs.push(e);
            existing.insert(edge_id(&e));
            *tc.entry(e.a).or_insert(0) += 1;
            *sc.entry(e.b).or_insert(0) += 1;
            if generated_stabiliser_ids.contains(&arena[e.a].support_id)
                || generated_stabiliser_ids.contains(&arena[e.b].support_id)
            {
                pair_distance_overrides.insert(edge_id(&e));
            }
        };

        for &sb in &extra {
            let mut candidates: Vec<Edge> = candidate_edges
                .iter()
                .copied()
                .filter(|e| e.b == sb)
                .collect();
            candidates.sort_by(|x, y| js_compare(x.h_dist - y.h_dist));
            let assigned = assigned_host_by_stabiliser.get(&sb);
            let on_host = |e: &Edge| assigned.is_some_and(|h| &arena[e.a].support_id == h);
            let fresh =
                |e: &Edge, existing: &HashSet<(String, String)>| !existing.contains(&edge_id(e));
            let chosen = candidates
                .iter()
                .find(|e| {
                    on_host(e)
                        && fresh(e, &existing_edge_ids)
                        && can_take(&trunk_edge_count, &stabiliser_edge_count, e)
                })
                .or_else(|| {
                    candidates
                        .iter()
                        .find(|e| on_host(e) && fresh(e, &existing_edge_ids))
                })
                .or_else(|| {
                    candidates.iter().find(|e| {
                        fresh(e, &existing_edge_ids)
                            && can_take(&trunk_edge_count, &stabiliser_edge_count, e)
                    })
                })
                .or_else(|| candidates.iter().find(|e| fresh(e, &existing_edge_ids)))
                .copied();
            if let Some(e) = chosen {
                add_edge(
                    e,
                    pairs,
                    &mut existing_edge_ids,
                    &mut trunk_edge_count,
                    &mut stabiliser_edge_count,
                );
            }
        }

        for &trunk in &group_trunks {
            let tid = &arena[trunk].support_id;
            let axes: Vec<f64> = pairs
                .iter()
                .filter(|e| &arena[e.a].support_id == tid || &arena[e.b].support_id == tid)
                .map(|e| e.angle_rad)
                .collect();
            if has_qualified_two_axis_bracing(
                &axes,
                auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG,
            ) {
                continue;
            }
            let mut best: Option<Edge> = None;
            let mut best_score = -1.0;
            for cand in &candidate_edges {
                if &arena[cand.a].support_id != tid
                    || existing_edge_ids.contains(&edge_id(cand))
                    || !can_take(&trunk_edge_count, &stabiliser_edge_count, cand)
                {
                    continue;
                }
                if axes.is_empty() {
                    best = Some(*cand);
                    break;
                }
                for &existing in &axes {
                    let sep = axis_separation_deg(existing, cand.angle_rad);
                    if sep >= auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG {
                        let score = 90.0 - (90.0 - sep).abs();
                        if score > best_score {
                            best_score = score;
                            best = Some(*cand);
                        }
                    }
                }
            }
            if let Some(e) = best {
                add_edge(
                    e,
                    pairs,
                    &mut existing_edge_ids,
                    &mut trunk_edge_count,
                    &mut stabiliser_edge_count,
                );
            }
        }
    }

    // ── Ladder ──
    let mut ladder = Ladder {
        arena: &arena,
        settings: &settings,
        grid,
        meshes,
        max_run,
        pair_distance_overrides: &pair_distance_overrides,
        brace_profile: build_brace_profile(settings.brace_diameter_mm),
        brace_ids: kept_braces.keys().cloned().collect(),
        knot_ids: next_knots.keys().cloned().collect(),
        brace_id_factory: UniqueIds {
            prefix: "auto-brace",
            index: 1,
        },
        knot_id_factory: UniqueIds {
            prefix: "auto-brace-knot",
            index: 1,
        },
        generated_braces: IndexMap::new(),
        generated_knots: IndexMap::new(),
        braced_axes_by_trunk_id: HashMap::new(),
    };
    for (model_id, pairs) in &ladder_pairs {
        let max_z = (0..trunk_count)
            .filter(|&i| &arena[i].model_id == model_id)
            .fold(f64::NEG_INFINITY, |m, i| {
                js_max(m, arena[i].top_reference_z)
            });
        lay_ladder(&mut ladder, pairs, &settings, &arena, max_z);
    }
    if settings.brace_tall_shafts {
        let kept_knots = kept_braces
            .values()
            .flat_map(|b| [&b.start_knot_id, &b.end_knot_id])
            .filter_map(|id| next_knots.get(id));
        brace_tall_shafts(&mut ladder, &arena, trunk_count, &settings, kept_knots);
    }
    let Ladder {
        mut generated_braces,
        mut generated_knots,
        braced_axes_by_trunk_id,
        ..
    } = ladder;

    // ── Post-ladder reconciliation: drop kickstands the ladder made redundant ──
    if !generated_stabilisers.is_empty() && !braced_axes_by_trunk_id.is_empty() {
        let mut drops: HashSet<&str> = HashSet::new();
        for build in &generated_stabilisers {
            let Some(host_trunk_id) =
                segment_owner_trunk_id.get(build.kickstand.host_segment_id.as_str())
            else {
                continue;
            };
            let axes = braced_axes_by_trunk_id
                .get(*host_trunk_id)
                .map_or(&[][..], Vec::as_slice);
            if has_qualified_two_axis_bracing(
                axes,
                auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG,
            ) {
                drops.insert(build.kickstand.base.id.as_str());
            }
        }
        if !drops.is_empty() {
            let mut dropped_segment_ids: HashSet<String> = HashSet::new();
            for id in &drops {
                if let Some(ks) = stabiliser_state.kickstands.get(*id) {
                    dropped_segment_ids.extend(ks.segments.iter().map(|s| s.id.clone()));
                }
            }
            let mut kept = StabiliserState::default();
            for (id, ks) in &stabiliser_state.kickstands {
                if drops.contains(id.as_str()) {
                    continue;
                }
                kept.kickstands.insert(id.clone(), ks.clone());
                if let Some(root) = stabiliser_state.roots.get(&ks.root_id) {
                    kept.roots.insert(root.base.id.clone(), root.clone());
                }
                if let Some(knot) = stabiliser_state.knots.get(&ks.host_knot_id) {
                    kept.knots.insert(knot.id.clone(), knot.clone());
                }
            }
            stabiliser_state = kept;

            let brace_ids: Vec<String> = generated_braces.keys().cloned().collect();
            for b_id in brace_ids {
                let brace = &generated_braces[&b_id];
                let (sk_id, ek_id) = (brace.start_knot_id.clone(), brace.end_knot_id.clone());
                let touches = |kid: &str| {
                    generated_knots
                        .get(kid)
                        .is_some_and(|k| dropped_segment_ids.contains(&k.parent_shaft_id))
                };
                if touches(&sk_id) || touches(&ek_id) {
                    generated_braces.shift_remove(&b_id);
                    generated_knots.shift_remove(&sk_id);
                    generated_knots.shift_remove(&ek_id);
                }
            }
        }
    }

    let generated_brace_count = generated_braces.len();
    let removed_brace_count = c.braces.len();
    let changed = generated_brace_count > 0 || removed_brace_count > 0;

    let mut next = snapshot.clone();
    let mut knots = next_knots;
    knots.extend(generated_knots);
    let mut braces = kept_braces;
    braces.extend(generated_braces);
    next.collections.braces = braces;
    // SoulCrafted: a root holding neither a trunk nor a kickstand goes. A
    // plate of several models is braced once per model, and each pass makes
    // every auto kickstand again: the roots of those it dropped stayed on the
    // raft as feet with nothing above them.
    let held: HashSet<&str> = c
        .trunks
        .values()
        .map(|t| t.root_id.as_str())
        .chain(
            stabiliser_state
                .kickstands
                .values()
                .map(|k| k.root_id.as_str()),
        )
        .collect();
    next.collections
        .roots
        .retain(|id, _| held.contains(id.as_str()));
    next.collections.kickstands = stabiliser_state.kickstands.clone();
    for k in stabiliser_state.kickstands.values() {
        if let Some(root) = stabiliser_state.roots.get(&k.root_id) {
            next.collections
                .roots
                .insert(root.base.id.clone(), root.clone());
        }
        if let Some(knot) = stabiliser_state.knots.get(&k.host_knot_id) {
            knots.insert(knot.id.clone(), knot.clone());
        }
    }
    next.collections.knots = knots;
    next.selected_id = if selected_stabiliser_cleared {
        None
    } else {
        next_selected
    };

    let skipped = trunk_count - grouped_ids.len();
    (
        next,
        AutoBraceResult {
            generated_brace_count,
            removed_brace_count,
            skipped_support_count: skipped,
            changed,
            status: if changed {
                AutoBraceStatus::Complete
            } else {
                AutoBraceStatus::NoEligibleSupports
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supports::types::{Joint, Roots, Transform, Trunk};

    /// A trunk standing at `(x, y)`, through `joints` (its segments' tops,
    /// in order), its segment ids `{id}-0`, `{id}-1`, ...
    fn trunk(state: &mut SupportState, id: &str, x: f64, y: f64, joints: &[[f64; 3]]) {
        let base = |id: String| SupportEntity {
            id,
            model_id: "model".to_owned(),
            type_id: Some(SupportTypeId::Trunk),
            ..SupportEntity::default()
        };
        let root = format!("{id}-root");
        state.collections.roots.insert(
            root.clone(),
            Roots {
                base: base(root.clone()),
                transform: Transform {
                    pos: Vec3::new(x, y, 0.0),
                    ..Transform::default()
                },
                diameter: 3.0,
                disk_height: 0.5,
                cone_height: 1.0,
                ..Roots::default()
            },
        );
        let mut below: Option<Joint> = None;
        let segments = joints
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut segment = Segment::straight(format!("{id}-{i}"), 1.0);
                let top = Joint {
                    id: format!("{id}-joint-{i}"),
                    pos: Vec3::new(p[0], p[1], p[2]),
                    diameter: 1.1,
                };
                segment.bottom_joint = below.replace(top.clone());
                segment.top_joint = Some(top);
                segment
            })
            .collect();
        state.collections.trunks.insert(
            id.to_owned(),
            Trunk {
                base: base(id.to_owned()),
                root_id: root,
                segments,
                ..Trunk::default()
            },
        );
    }

    /// A row of trunks along X: two tall ones 4 mm apart, a short one
    /// between them and one beyond each, the short ones 8 mm high. The tall
    /// ones bend at 15 mm and lean side by side towards +Y up to 29 mm, as
    /// the two long diagonals up a mini's staff: how many braces end on
    /// their bent segments. The pairs below join each tall trunk to the
    /// short ones next to it, along the row, and never the two tall ones.
    fn braces_on_the_bends(brace_tall_shafts: bool) -> usize {
        let mut state = SupportState::default();
        trunk(&mut state, "short-a", 0.0, 0.0, &[[0.0, 0.0, 8.0]]);
        trunk(
            &mut state,
            "tall-a",
            2.5,
            0.0,
            &[[2.5, 0.0, 15.0], [2.5, 14.0, 29.0]],
        );
        trunk(&mut state, "short-m", 4.5, 0.0, &[[4.5, 0.0, 8.0]]);
        trunk(
            &mut state,
            "tall-b",
            6.5,
            0.0,
            &[[6.5, 0.0, 15.0], [6.5, 14.0, 29.0]],
        );
        trunk(&mut state, "short-b", 9.0, 0.0, &[[9.0, 0.0, 8.0]]);
        let settings = AutoBracingSettings {
            brace_tall_shafts,
            ..AutoBracingSettings::default()
        };
        let mut next = 0;
        let (braced, _) = build_auto_braced_snapshot(
            &state,
            &settings,
            &SupportSettings::default(),
            &BraceMeshes::default(),
            &state,
            &mut || {
                next += 1;
                format!("id-{next}")
            },
        );
        let c = &braced.collections;
        c.braces
            .values()
            .flat_map(|b| [&b.start_knot_id, &b.end_knot_id])
            .filter_map(|id| c.knots.get(id))
            .filter(|k| k.parent_shaft_id == "tall-a-1" || k.parent_shaft_id == "tall-b-1")
            .count()
    }

    /// #464: the bent tops of two tall trunks, held only by short ones, are
    /// braced to each other where they run side by side; the port's rule
    /// leaves them bare.
    #[test]
    fn tall_bent_trunks_are_braced_where_they_run_side_by_side() {
        assert_eq!(braces_on_the_bends(false), 0, "the port's rule");
        assert!(braces_on_the_bends(true) >= 2, "braced up there");
    }
}

//! A bracing run over a whole plate.
//!
//! The shafts are split into groups and paired model-wide; kickstands are
//! built for tall shafts the pairs leave held along fewer than two axes, and
//! paired to the shafts they lean on; the braces climb every pair; tall
//! shaft tops left bare get braced to their neighbours; and a kickstand the
//! braces made unnecessary is taken out again, with its braces.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::grouping::{GroupNode, GroupingSettings, group_shafts};
pub use super::hand::{HandBrace, HandBraceOutcome, HandBraceRefusal, brace_new_shaft, hand_brace};
use super::ladder::{BraceWriter, brace_tall_shafts, lay_ladder};
use super::pairing::{
    Edge, bracing_grid, is_cardinal_delta, min_pair_span_mm, model_pairs, pair_key, plan_point,
};
use super::samples::{EPS, Shaft, collect_shafts, shared_mid_z, span_at};
use bread_support_geom::geom::math_utils::{js_atan2, js_max};
use bread_support_model::auto_bracing::mesh_clearance::BraceMeshes;
use bread_support_model::auto_bracing::settings::{
    AutoBracingSettings, auto_bracing_hard_rules, normalize_auto_bracing_settings,
};
use bread_support_model::auto_bracing::two_axis_detection::{
    axis_separation_deg, has_qualified_two_axis_bracing, normalize_axis_angle_rad,
};
use bread_support_model::settings::types::SupportSettings;
pub use bread_support_model::support_kinds::store_type_of;
use bread_support_model::support_kinds::{
    is_auto_braceable_shaft_type, is_lateral_stabiliser_type, parse_prefixed_segment_id,
};
use bread_support_model::types::{
    Brace, Kickstand, Knot, SupportCollections, SupportGeneratedBy, SupportState,
};
use bread_support_placement::auto_support::types::js_compare;
use bread_support_placement::support_types::kickstand::kickstand_stabiliser::{
    BraceAxisEdge, GridSnap, StabiliserState, generate_required_kickstands,
};

/// Most kickstand pairs one shaft takes, and one kickstand.
const KICKSTAND_PAIRS_PER_SHAFT: usize = 2;
const KICKSTAND_PAIRS_PER_KICKSTAND: usize = 2;

const SECOND_AXIS_DEG: f64 = auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG;

/// How a bracing run ended, as a code the UI words.
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

fn made_by_run(generated_by: Option<SupportGeneratedBy>) -> bool {
    generated_by == Some(SupportGeneratedBy::AutoBracing)
}

/// The kickstands of `state` that `keep` accepts, with their roots and the
/// knots they lean on.
fn kickstands_where(
    state: &StabiliserState,
    keep: impl Fn(&str, &Kickstand) -> bool,
) -> StabiliserState {
    let mut out = StabiliserState::default();
    for (id, k) in &state.kickstands {
        if !keep(id, k) {
            continue;
        }
        out.kickstands.insert(id.clone(), k.clone());
        if let Some(root) = state.roots.get(&k.root_id) {
            out.roots.insert(root.base.id.clone(), root.clone());
        }
        if let Some(knot) = state.knots.get(&k.host_knot_id) {
            out.knots.insert(knot.id.clone(), knot.clone());
        }
    }
    out
}

/// The kickstands a run starts from: all of `snapshot`'s with every root and
/// knot, or, when an earlier run made some, the others with their own roots
/// and knots. Also whether that drops the selected kickstand.
fn starting_kickstands(snapshot: &SupportState) -> (StabiliserState, bool) {
    let c = &snapshot.collections;
    let everything = StabiliserState {
        kickstands: c.kickstands.clone(),
        roots: c.roots.clone(),
        knots: c.knots.clone(),
    };
    if !c
        .kickstands
        .values()
        .any(|k| made_by_run(k.base.generated_by))
    {
        return (everything, false);
    }
    let kept = kickstands_where(&everything, |_, k| !made_by_run(k.base.generated_by));
    let selection_dropped = snapshot
        .selected_id
        .as_ref()
        .is_some_and(|sel| c.kickstands.contains_key(sel) && !kept.kickstands.contains_key(sel));
    (kept, selection_dropped)
}

/// The nearest of the first `posts` shafts of `kick`'s model, measured at
/// the middle of the heights both span, if within `max_len`.
fn nearest_post(shafts: &[Shaft], posts: usize, kick: &Shaft, max_len: f64) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut best_dist = f64::INFINITY;
    for (i, post) in shafts[..posts].iter().enumerate() {
        if post.model_id != kick.model_id {
            continue;
        }
        let Some((h_dist, _)) = span_at(post, kick, shared_mid_z(post, kick)) else {
            continue;
        };
        if h_dist < best_dist {
            best_dist = h_dist;
            best = Some(i);
        }
    }
    best.filter(|_| !(best_dist > max_len + EPS))
}

/// What pairing kickstands needs to know of the run.
struct KickstandPairing<'r> {
    shafts: &'r [Shaft],
    /// Ids of the kickstands built this run.
    built: &'r HashSet<String>,
    /// The shaft each kickstand leans on, by id.
    host_of: &'r HashMap<usize, String>,
    grid: GridSnap,
    max_len: f64,
}

impl KickstandPairing<'_> {
    fn id(&self, s: usize) -> &str {
        &self.shafts[s].id
    }

    fn key(&self, e: &Edge) -> (String, String) {
        pair_key(self.id(e.a), self.id(e.b))
    }

    fn is_built(&self, s: usize) -> bool {
        self.built.contains(self.id(s))
    }

    /// The horizontal distance and axis from `a` to `b`: between grid
    /// points along a grid line when braces follow a grid, else at the
    /// middle of the heights both span.
    fn span(&self, a: usize, b: usize) -> Option<(f64, f64)> {
        let (a, b) = (&self.shafts[a], &self.shafts[b]);
        if !self.grid.enabled {
            return span_at(a, b, shared_mid_z(a, b));
        }
        let (pa, pb) = (plan_point(a, self.grid), plan_point(b, self.grid));
        let (dx, dy) = (pb.x - pa.x, pb.y - pa.y);
        if !is_cardinal_delta(dx, dy, self.grid.spacing_mm) {
            return None;
        }
        let h_dist = (dx * dx + dy * dy).sqrt();
        if h_dist < EPS {
            return None;
        }
        Some((h_dist, normalize_axis_angle_rad(js_atan2(dy, dx))))
    }

    /// The pair (`a`, `b`) when within reach, which a kickstand built this
    /// run does not need to be.
    fn link(&self, a: usize, b: usize, exempt: bool) -> Option<Edge> {
        let (h_dist, angle_rad) = self.span(a, b)?;
        if h_dist > self.max_len + EPS && !exempt {
            return None;
        }
        Some(Edge {
            a,
            b,
            h_dist,
            angle_rad,
        })
    }

    /// Adds to `pairs` (its model's) the kickstands of group `members`:
    /// each kickstand to a shaft (the one it leans on if it can) or another
    /// kickstand, then each shaft of the group still short of two axes to a
    /// kickstand nearest square to its axes. Pairs with a kickstand built
    /// this run join `unlimited`.
    fn pair_group(
        &self,
        members: &[usize],
        pairs: &mut Vec<Edge>,
        unlimited: &mut HashSet<(String, String)>,
    ) {
        let shafts = self.shafts;
        let posts: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&s| is_auto_braceable_shaft_type(shafts[s].kind))
            .collect();
        let kicks: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&s| is_lateral_stabiliser_type(shafts[s].kind))
            .collect();
        if kicks.is_empty() || posts.is_empty() {
            return;
        }
        let mut candidates: Vec<Edge> = Vec::new();
        for &k in &kicks {
            for &p in &posts {
                candidates.extend(self.link(p, k, self.is_built(k)));
            }
        }
        for (i, &k1) in kicks.iter().enumerate() {
            for &k2 in &kicks[i + 1..] {
                candidates.extend(self.link(k1, k2, self.is_built(k1) || self.is_built(k2)));
            }
        }

        let mut taken: HashSet<(String, String)> = pairs.iter().map(|e| self.key(e)).collect();
        // Pairs so far in this group, by the first shaft and by the second.
        let mut as_first: HashMap<usize, usize> = HashMap::new();
        let mut as_second: HashMap<usize, usize> = HashMap::new();
        let room =
            |as_first: &HashMap<usize, usize>, as_second: &HashMap<usize, usize>, e: &Edge| {
                as_first.get(&e.a).copied().unwrap_or(0) < KICKSTAND_PAIRS_PER_SHAFT
                    && as_second.get(&e.b).copied().unwrap_or(0) < KICKSTAND_PAIRS_PER_KICKSTAND
            };
        let mut add = |e: Edge,
                       pairs: &mut Vec<Edge>,
                       taken: &mut HashSet<(String, String)>,
                       as_first: &mut HashMap<usize, usize>,
                       as_second: &mut HashMap<usize, usize>| {
            pairs.push(e);
            taken.insert(self.key(&e));
            *as_first.entry(e.a).or_insert(0) += 1;
            *as_second.entry(e.b).or_insert(0) += 1;
            if self.is_built(e.a) || self.is_built(e.b) {
                unlimited.insert(self.key(&e));
            }
        };

        // Each kickstand: preferably to the shaft it leans on, preferably to
        // a shaft with room, nearest first.
        for &k in &kicks {
            let mut own: Vec<Edge> = candidates.iter().copied().filter(|e| e.b == k).collect();
            own.sort_by(|x, y| js_compare(x.h_dist - y.h_dist));
            let host = self.host_of.get(&k);
            let on_host = |e: &Edge| host.is_some_and(|h| self.id(e.a) == h);
            let fresh = |e: &Edge, taken: &HashSet<(String, String)>| !taken.contains(&self.key(e));
            let roomy = |e: &Edge| room(&as_first, &as_second, e);
            let pick = own
                .iter()
                .find(|e| on_host(e) && fresh(e, &taken) && roomy(e))
                .or_else(|| own.iter().find(|e| on_host(e) && fresh(e, &taken)))
                .or_else(|| own.iter().find(|e| fresh(e, &taken) && roomy(e)))
                .or_else(|| own.iter().find(|e| fresh(e, &taken)))
                .copied();
            if let Some(e) = pick {
                add(e, pairs, &mut taken, &mut as_first, &mut as_second);
            }
        }

        // Each shaft still short of two axes: the kickstand pair nearest
        // square to one of its axes (or the first, when it has none).
        for &post in &posts {
            let me = self.id(post);
            let axes: Vec<f64> = pairs
                .iter()
                .filter(|e| self.id(e.a) == me || self.id(e.b) == me)
                .map(|e| e.angle_rad)
                .collect();
            if has_qualified_two_axis_bracing(&axes, SECOND_AXIS_DEG) {
                continue;
            }
            let mut best: Option<Edge> = None;
            let mut best_score = -1.0;
            for e in &candidates {
                if self.id(e.a) != me
                    || taken.contains(&self.key(e))
                    || !room(&as_first, &as_second, e)
                {
                    continue;
                }
                if axes.is_empty() {
                    best = Some(*e);
                    break;
                }
                for &axis in &axes {
                    let sep = axis_separation_deg(axis, e.angle_rad);
                    if sep >= SECOND_AXIS_DEG {
                        let score = 90.0 - (90.0 - sep).abs();
                        if score > best_score {
                            best_score = score;
                            best = Some(*e);
                        }
                    }
                }
            }
            if let Some(e) = best {
                add(e, pairs, &mut taken, &mut as_first, &mut as_second);
            }
        }
    }
}

/// Braces the supports of `snapshot` and returns the braced state. `live`
/// gives the grid and kickstand sizing; `store` (the committed state) gives
/// the kind of an entity without one stamped; kickstand ids come from
/// `new_id`.
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
    let grid = bracing_grid(live);
    let max_len = settings.max_brace_length_mm;

    let mut shafts: Vec<Shaft> = collect_shafts(c, &store.collections)
        .into_iter()
        .filter(|s| is_auto_braceable_shaft_type(s.kind))
        .collect();
    // Models braced together pair as one (#475).
    if !settings.brace_together.is_empty() {
        for shaft in &mut shafts {
            shaft.model_id = settings.bracing_key(&shaft.model_id).to_owned();
        }
    }
    // The shafts that are not kickstands: the first `posts` of `shafts`.
    let posts = shafts.len();
    if posts < auto_bracing_hard_rules::MIN_GROUP_SIZE {
        let result = AutoBraceResult {
            generated_brace_count: 0,
            removed_brace_count: 0,
            skipped_support_count: posts,
            changed: false,
            status: AutoBraceStatus::NoEligibleSupports,
        };
        return (snapshot.clone(), result);
    }

    let (mut kickstands, selection_dropped) = starting_kickstands(snapshot);

    let index_of: HashMap<String, usize> = shafts
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();
    let nodes: Vec<GroupNode> = shafts
        .iter()
        .map(|s| GroupNode {
            support_id: s.id.clone(),
            model_id: s.model_id.clone(),
            point: plan_point(s, grid),
        })
        .collect();
    let group_ids = group_shafts(
        &nodes,
        &GroupingSettings {
            seed_spacing_mm: settings.seed_spacing_mm,
            seed_jitter_mm: settings.seed_jitter_mm,
            max_neighbor_distance_mm: max_len,
        },
    );

    // Each model's pairs, models in order of first appearance.
    let mut members_of: IndexMap<String, Vec<usize>> = IndexMap::new();
    for (i, s) in shafts.iter().enumerate() {
        members_of.entry(s.model_id.clone()).or_default().push(i);
    }
    let min_span = min_pair_span_mm(&settings);
    let mut pairs_of: IndexMap<String, Vec<Edge>> = members_of
        .iter()
        .map(|(model, members)| {
            let pairs = model_pairs(&shafts, members, max_len, grid, min_span);
            (model.clone(), pairs)
        })
        .collect();

    // Kickstands for tall shafts these pairs leave short of two axes.
    let axis_edges: Vec<BraceAxisEdge> = pairs_of
        .values()
        .flatten()
        .map(|e| BraceAxisEdge {
            a: shafts[e.a].id.clone(),
            b: shafts[e.b].id.clone(),
            angle_rad: e.angle_rad,
        })
        .collect();
    let mut built = generate_required_kickstands(
        snapshot,
        &kickstands,
        &settings,
        &axis_edges,
        grid,
        meshes,
        live,
        store,
        new_id,
    );
    let mut built_ids: HashSet<String> = HashSet::new();
    for build in &mut built {
        if build.kickstand.host_segment_id.is_empty() {
            build.kickstand.host_segment_id = build.host_knot.parent_shaft_id.clone();
        }
        build.kickstand.base.generated_by = Some(SupportGeneratedBy::AutoBracing);
        built_ids.insert(build.kickstand.base.id.clone());
        kickstands
            .kickstands
            .insert(build.kickstand.base.id.clone(), build.kickstand.clone());
        kickstands
            .roots
            .insert(build.root.base.id.clone(), build.root.clone());
        kickstands
            .knots
            .insert(build.host_knot.id.clone(), build.host_knot.clone());
    }

    // The kickstands join the shafts, after the posts.
    let kick_collections = SupportCollections {
        kickstands: kickstands.kickstands.clone(),
        roots: kickstands.roots.clone(),
        knots: kickstands.knots.clone(),
        ..SupportCollections::default()
    };
    shafts.extend(collect_shafts(&kick_collections, &store.collections));
    let shafts = shafts;

    let mut trunk_of_segment: HashMap<&str, &str> = HashMap::new();
    for t in c.trunks.values() {
        for seg in &t.segments {
            trunk_of_segment.insert(seg.id.as_str(), t.base.id.as_str());
        }
    }

    // Each kickstand's host: the trunk it leans on, else the nearest post.
    let mut host_of: HashMap<usize, String> = HashMap::new();
    let mut leaning_on: HashMap<String, Vec<usize>> = HashMap::new();
    for k in posts..shafts.len() {
        let kick = &shafts[k];
        let host = kick
            .host_segment_id
            .as_deref()
            .and_then(|seg| trunk_of_segment.get(seg))
            .map(|t| (*t).to_owned())
            .or_else(|| nearest_post(&shafts, posts, kick, max_len).map(|i| shafts[i].id.clone()));
        let Some(host) = host else {
            continue;
        };
        host_of.insert(k, host.clone());
        leaning_on.entry(host).or_default().push(k);
    }

    // The groups, each followed by the kickstands leaning on its shafts.
    let groups: Vec<Vec<usize>> = group_ids
        .iter()
        .map(|ids| {
            let group_posts: Vec<usize> = ids
                .iter()
                .filter_map(|id| index_of.get(id).copied())
                .collect();
            let mut members = group_posts.clone();
            for &p in &group_posts {
                if let Some(kicks) = leaning_on.get(&shafts[p].id) {
                    members.extend(kicks.iter().copied());
                }
            }
            members
        })
        .collect();
    let grouped_posts: HashSet<&str> = groups
        .iter()
        .flatten()
        .filter(|&&s| is_auto_braceable_shaft_type(shafts[s].kind))
        .map(|&s| shafts[s].id.as_str())
        .collect();

    // The braces kept: all but those an earlier run made, when asked to
    // drop them; and the knots kept: all but those of dropped braces that
    // nothing else hangs from.
    let kept_braces: IndexMap<String, Brace> = c
        .braces
        .iter()
        .filter(|(_, b)| !(made_by_run(b.base.generated_by) && settings.remove_existing_bracing))
        .map(|(id, b)| (id.clone(), b.clone()))
        .collect();
    let brace_knots: HashSet<&str> = c
        .braces
        .values()
        .flat_map(|b| [b.start_knot_id.as_str(), b.end_knot_id.as_str()])
        .collect();
    let mut still_hung: HashSet<&str> = HashSet::new();
    still_hung.extend(c.branches.values().map(|e| e.parent_knot_id.as_str()));
    still_hung.extend(c.leaves.values().map(|e| e.parent_knot_id.as_str()));
    still_hung.extend(
        kept_braces
            .values()
            .flat_map(|b| [b.start_knot_id.as_str(), b.end_knot_id.as_str()]),
    );
    still_hung.extend(c.kickstands.values().map(|e| e.host_knot_id.as_str()));
    let kept_knots: IndexMap<String, Knot> = c
        .knots
        .iter()
        .filter(|(id, _)| !brace_knots.contains(id.as_str()) || still_hung.contains(id.as_str()))
        .map(|(id, k)| (id.clone(), k.clone()))
        .collect();

    // A selected brace that was dropped is no longer selected.
    let selected_brace = snapshot
        .selected_id
        .as_deref()
        .map(|sel| parse_prefixed_segment_id(sel).map_or(sel, |(_, entity)| entity));
    let brace_selection_dropped =
        selected_brace.is_some_and(|id| c.braces.contains_key(id) && !kept_braces.contains_key(id));

    // Kickstands join their model's pairs, group by group.
    let mut unlimited: HashSet<(String, String)> = HashSet::new();
    let kick_pairing = KickstandPairing {
        shafts: &shafts,
        built: &built_ids,
        host_of: &host_of,
        grid,
        max_len,
    };
    for members in &groups {
        let Some(&first_post) = members
            .iter()
            .find(|&&s| is_auto_braceable_shaft_type(shafts[s].kind))
        else {
            continue;
        };
        if let Some(pairs) = pairs_of.get_mut(&shafts[first_post].model_id) {
            kick_pairing.pair_group(members, pairs, &mut unlimited);
        }
    }

    let mut writer = BraceWriter::new(
        &shafts,
        &settings,
        grid,
        meshes,
        &unlimited,
        kept_braces.keys().cloned().collect(),
        kept_knots.keys().cloned().collect(),
    );
    for (model, pairs) in &pairs_of {
        let max_z = shafts[..posts]
            .iter()
            .filter(|s| &s.model_id == model)
            .fold(f64::NEG_INFINITY, |m, s| js_max(m, s.top_z()));
        lay_ladder(&mut writer, pairs, max_z);
    }
    let held = kept_braces
        .values()
        .flat_map(|b| [&b.start_knot_id, &b.end_knot_id])
        .filter_map(|id| kept_knots.get(id));
    brace_tall_shafts(&mut writer, posts, held);
    let (mut new_braces, mut new_knots, axes) = writer.into_parts();

    // A kickstand built this run whose trunk the braces now hold along two
    // axes goes, with the braces on it.
    if !built.is_empty() && !axes.is_empty() {
        let unneeded: HashSet<&str> = built
            .iter()
            .filter(|build| {
                trunk_of_segment
                    .get(build.kickstand.host_segment_id.as_str())
                    .is_some_and(|trunk| {
                        let held_along = axes.get(*trunk).map_or(&[][..], Vec::as_slice);
                        has_qualified_two_axis_bracing(held_along, SECOND_AXIS_DEG)
                    })
            })
            .map(|build| build.kickstand.base.id.as_str())
            .collect();
        if !unneeded.is_empty() {
            let gone_segments: HashSet<String> = unneeded
                .iter()
                .filter_map(|id| kickstands.kickstands.get(*id))
                .flat_map(|k| k.segments.iter().map(|s| s.id.clone()))
                .collect();
            kickstands = kickstands_where(&kickstands, |id, _| !unneeded.contains(id));
            let ids: Vec<String> = new_braces.keys().cloned().collect();
            for id in ids {
                let (start, end) = {
                    let brace = &new_braces[&id];
                    (brace.start_knot_id.clone(), brace.end_knot_id.clone())
                };
                let on_gone = |knot: &str| {
                    new_knots
                        .get(knot)
                        .is_some_and(|k| gone_segments.contains(&k.parent_shaft_id))
                };
                if on_gone(&start) || on_gone(&end) {
                    new_braces.shift_remove(&id);
                    new_knots.shift_remove(&start);
                    new_knots.shift_remove(&end);
                }
            }
        }
    }

    let generated_brace_count = new_braces.len();
    let removed_brace_count = c.braces.len();
    let changed = generated_brace_count > 0 || removed_brace_count > 0;

    let mut next = snapshot.clone();
    let mut knots = kept_knots;
    knots.extend(new_knots);
    let mut braces = kept_braces;
    braces.extend(new_braces);
    next.collections.braces = braces;
    // A root holding neither a trunk nor a kickstand goes: a plate of
    // several models is braced once per model, each pass rebuilding the
    // run's kickstands, and the roots of those it dropped would stay on the
    // raft as feet with nothing above them.
    let rooted: HashSet<&str> = c
        .trunks
        .values()
        .map(|t| t.root_id.as_str())
        .chain(kickstands.kickstands.values().map(|k| k.root_id.as_str()))
        .collect();
    next.collections
        .roots
        .retain(|id, _| rooted.contains(id.as_str()));
    next.collections.kickstands = kickstands.kickstands.clone();
    for k in kickstands.kickstands.values() {
        if let Some(root) = kickstands.roots.get(&k.root_id) {
            next.collections
                .roots
                .insert(root.base.id.clone(), root.clone());
        }
        if let Some(knot) = kickstands.knots.get(&k.host_knot_id) {
            knots.insert(knot.id.clone(), knot.clone());
        }
    }
    next.collections.knots = knots;
    if selection_dropped || brace_selection_dropped {
        next.selected_id = None;
    }

    let result = AutoBraceResult {
        generated_brace_count,
        removed_brace_count,
        skipped_support_count: posts - grouped_posts.len(),
        changed,
        status: if changed {
            AutoBraceStatus::Complete
        } else {
            AutoBraceStatus::NoEligibleSupports
        },
    };
    (next, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bread_support_model::support_kinds::SupportTypeId;
    use bread_support_model::types::{Joint, Roots, Segment, SupportEntity, Transform, Trunk, Vec3};

    /// Adds a trunk standing at (`x`, `y`) whose segments end at `joints`,
    /// bottom to top; its segments are `{id}-0`, `{id}-1`, ...
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

    /// Five trunks in a row along X: two tall ones 4 mm apart with a short
    /// one between them and one beyond each, the short ones 8 mm tall. The
    /// tall ones bend at 15 mm and lean side by side towards +Y up to 29 mm,
    /// like two long diagonals up a figure's staff. The pairs below join
    /// each tall trunk to its short neighbours, never to the other tall one.
    /// Returns how many brace ends land on the bent segments.
    fn brace_ends_on_the_bends() -> usize {
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
        let mut counter = 0;
        let (braced, _) = build_auto_braced_snapshot(
            &state,
            &AutoBracingSettings::default(),
            &SupportSettings::default(),
            &BraceMeshes::default(),
            &state,
            &mut || {
                counter += 1;
                format!("id-{counter}")
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
    /// braced to each other where they run side by side.
    #[test]
    fn tall_bent_trunks_are_braced_where_they_run_side_by_side() {
        assert!(brace_ends_on_the_bends() >= 2, "braced up there");
    }
}

//! The braces up a pair of shafts. Every brace climbs at 45° or steeper from
//! a low end on one shaft to a high end on the other; the ladder decides at
//! which heights and which way, and the writer checks each brace (reach,
//! grid, length, model clearance) before adding it with its two knots.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::pairing::{Edge, is_cardinal_delta, min_pair_span_mm, pair_key};
use super::samples::{Anchor, EPS, Shaft, anchor_at, plan_distance, span_at};
use support_geom::geom::math_utils::{js_atan2, js_max, js_min};
use support_model::auto_bracing::mesh_clearance::{
    BraceMeshes, line_passes_mesh_clearance, line_stays_off_mesh,
};
use support_model::auto_bracing::settings::{
    AutoBracingPattern, AutoBracingSettings, GridDiagonal, StackedBracing, auto_bracing_hard_rules,
};
use support_model::auto_bracing::two_axis_detection::{
    axis_separation_deg, normalize_axis_angle_rad,
};
use support_model::auto_support::zone_rules::BRACE_SHARE_OF_TRUNK;
use support_model::constants::JOINT_DIAMETER_OFFSET_MM;
use support_model::support_kinds::{SupportTypeId, is_auto_braceable_shaft_type};
use support_model::types::{
    Brace, BraceDebugSection, BraceProfile, Knot, SupportEntity, SupportGeneratedBy,
};
use support_placement::auto_support::types::js_compare;
use support_placement::support_types::kickstand::kickstand_stabiliser::GridSnap;

/// Ids `{prefix}-1`, `{prefix}-2`, ... skipping those already taken.
struct IdPool {
    prefix: &'static str,
    next: usize,
    taken: HashSet<String>,
}

impl IdPool {
    fn new(prefix: &'static str, taken: HashSet<String>) -> Self {
        IdPool {
            prefix,
            next: 1,
            taken,
        }
    }

    fn draw(&mut self) -> String {
        loop {
            let id = format!("{}-{}", self.prefix, self.next);
            self.next += 1;
            if self.taken.insert(id.clone()) {
                return id;
            }
        }
    }
}

/// Lays braces between the shafts of a run and keeps what it laid.
pub(super) struct BraceWriter<'a> {
    shafts: &'a [Shaft],
    settings: &'a AutoBracingSettings,
    grid: GridSnap,
    meshes: &'a BraceMeshes<'a>,
    /// Pairs exempt from the length and grid limits (those with a kickstand
    /// built this run).
    unlimited: &'a HashSet<(String, String)>,
    brace_ids: IdPool,
    knot_ids: IdPool,
    pub braces: IndexMap<String, Brace>,
    pub knots: IndexMap<String, Knot>,
    /// Per shaft, the axes of the shaft-to-shaft braces laid on it.
    pub axes: HashMap<String, Vec<f64>>,
}

impl<'a> BraceWriter<'a> {
    /// A writer whose ids skip `brace_ids` and `knot_ids`.
    pub(super) fn new(
        shafts: &'a [Shaft],
        settings: &'a AutoBracingSettings,
        grid: GridSnap,
        meshes: &'a BraceMeshes<'a>,
        unlimited: &'a HashSet<(String, String)>,
        brace_ids: HashSet<String>,
        knot_ids: HashSet<String>,
    ) -> Self {
        BraceWriter {
            shafts,
            settings,
            grid,
            meshes,
            unlimited,
            brace_ids: IdPool::new("auto-brace", brace_ids),
            knot_ids: IdPool::new("auto-brace-knot", knot_ids),
            braces: IndexMap::new(),
            knots: IndexMap::new(),
            axes: HashMap::new(),
        }
    }

    /// The braces laid, their knots, and each shaft's brace axes.
    #[allow(clippy::type_complexity)]
    pub(super) fn into_parts(
        self,
    ) -> (
        IndexMap<String, Brace>,
        IndexMap<String, Knot>,
        HashMap<String, Vec<f64>>,
    ) {
        (self.braces, self.knots, self.axes)
    }

    /// A brace from shaft `low` at height `z` up to shaft `high`, rising as
    /// far as it runs across (never less than `min_rise_mm`), when it fits.
    pub(super) fn place(
        &mut self,
        low: usize,
        high: usize,
        section: BraceDebugSection,
        z: f64,
        min_rise_mm: f64,
    ) {
        let (low_s, high_s) = (&self.shafts[low], &self.shafts[high]);
        let unlimited = self.unlimited.contains(&pair_key(&low_s.id, &high_s.id));
        let Some(from) = anchor_at(low_s, z) else {
            return;
        };
        let Some(level) = anchor_at(high_s, z) else {
            return;
        };
        // The rise that makes the brace 45°: the other shaft may lean, so
        // the run is measured again at the end's new height, a few times.
        let mut rise = js_max(min_rise_mm, plan_distance(from.pos, level.pos));
        for _ in 0..3 {
            let Some(end) = anchor_at(high_s, z + rise) else {
                return;
            };
            let next = js_max(min_rise_mm, plan_distance(from.pos, end.pos));
            let settled = (next - rise).abs() < 0.01;
            rise = next;
            if settled {
                break;
            }
            if rise < EPS {
                return;
            }
        }
        let max_len = self.settings.max_brace_length_mm;
        if !unlimited && rise > max_len + EPS {
            return;
        }
        // Both ends stay below the shafts' tops.
        let top = z + rise;
        if top >= low_s.top_z() - 0.1 || top >= high_s.top_z() - 0.1 {
            return;
        }
        let Some(to) = anchor_at(high_s, top) else {
            return;
        };
        let (dx, dy) = (to.pos.x - from.pos.x, to.pos.y - from.pos.y);
        if self.grid.enabled && !unlimited && !is_cardinal_delta(dx, dy, self.grid.spacing_mm) {
            return;
        }
        if !unlimited && (dx * dx + dy * dy).sqrt() > max_len + EPS {
            return;
        }
        // A brace keeps clear of every model braced with its own (#475).
        let diameter = self.settings.brace_diameter_mm;
        for model in self.settings.braced_with(&from.model_id) {
            let mesh = self.meshes.get(model);
            if !line_passes_mesh_clearance(from.pos, to.pos, mesh, diameter)
                || !line_stays_off_mesh(from.pos, to.pos, mesh, diameter)
            {
                return;
            }
        }
        self.commit(low, high, section, from, to);
    }

    /// Adds the brace from `from` (on shaft `low`) to `to` (on `high`) and
    /// its knots, unless a preset's end would land too low on its shaft.
    pub(super) fn commit(
        &mut self,
        low: usize,
        high: usize,
        section: BraceDebugSection,
        from: Anchor,
        to: Anchor,
    ) {
        let (low_s, high_s) = (&self.shafts[low], &self.shafts[high]);
        let settings = self.settings;
        // A preset's brace ends a diameter above where its shaft starts, or
        // the end would hang below the shaft.
        if settings.stacked.is_some() {
            let clear = settings.brace_diameter_mm;
            if from.pos.z < low_s.bottom_z() + clear || to.pos.z < high_s.bottom_z() + clear {
                return;
            }
        }
        let start_id = self.knot_ids.draw();
        let end_id = self.knot_ids.draw();
        let brace_id = self.brace_ids.draw();
        // The tessellation reads a brace's width off its knots. By default a
        // knot takes the width of the segment it sits on; a preset uses its
        // own diameter, capped by the thinner shaft so the end stays inside
        // it, and #471 a share of the thinner shaft.
        let thinner = from.host_diameter_mm.min(to.host_diameter_mm);
        let width = if settings.follow_trunks {
            Some(BRACE_SHARE_OF_TRUNK * thinner)
        } else if settings.stacked.is_some() {
            Some(settings.brace_diameter_mm.min(thinner))
        } else {
            None
        };
        let knot = |id: &str, at: &Anchor| Knot {
            id: id.to_owned(),
            parent_shaft_id: at.segment_id.clone(),
            t: Some(at.t),
            pos: at.pos,
            diameter: Some(width.unwrap_or(at.host_diameter_mm) + JOINT_DIAMETER_OFFSET_MM),
        };
        self.knots.insert(start_id.clone(), knot(&start_id, &from));
        self.knots.insert(end_id.clone(), knot(&end_id, &to));
        self.braces.insert(
            brace_id.clone(),
            Brace {
                base: SupportEntity {
                    id: brace_id,
                    model_id: from.model_id.clone(),
                    generated_by: Some(SupportGeneratedBy::AutoBracing),
                    ..SupportEntity::default()
                },
                start_knot_id: start_id,
                end_knot_id: end_id,
                placement_surface: None,
                curve: None,
                profile: BraceProfile {
                    diameter: settings.brace_diameter_mm,
                },
                debug_section: Some(section),
            },
        );
        // Only braces between two shafts count as an axis of either.
        if is_auto_braceable_shaft_type(low_s.kind) && is_auto_braceable_shaft_type(high_s.kind) {
            let dx = to.pos.x - from.pos.x;
            let dy = to.pos.y - from.pos.y;
            let axis = normalize_axis_angle_rad(js_atan2(dy, dx));
            for id in [&low_s.id, &high_s.id] {
                self.axes.entry(id.clone()).or_default().push(axis);
            }
        }
    }
}

fn section_of(first: bool) -> BraceDebugSection {
    if first {
        BraceDebugSection::Initial
    } else {
        BraceDebugSection::Repeating
    }
}

/// The ends of pair `index`, low end first: neighbouring pairs lean
/// opposite ways.
fn leaning(edge: &Edge, index: usize) -> (usize, usize) {
    if index.is_multiple_of(2) {
        (edge.a, edge.b)
    } else {
        (edge.b, edge.a)
    }
}

/// Most links one zigzag chain may have.
const MAX_ZIGZAG_LINKS: usize = 200;

/// Per pair, a zigzag from `start_z` to `max_z`: each link rises by the
/// pair's span (at least the zigzag floor) and starts where the last ended,
/// on the other shaft.
fn zigzag(
    writer: &mut BraceWriter<'_>,
    pairs: &[Edge],
    start_z: f64,
    max_z: f64,
    first_section: BraceDebugSection,
) {
    for (index, edge) in pairs.iter().enumerate() {
        if !(edge.h_dist > 0.0) {
            continue;
        }
        let rise = js_max(edge.h_dist, auto_bracing_hard_rules::MIN_ZIG_ZAG_RISE_MM);
        let (mut low, mut high) = leaning(edge, index);
        let mut z = start_z;
        let mut section = first_section;
        for _ in 0..MAX_ZIGZAG_LINKS {
            if !(z < max_z) {
                break;
            }
            writer.place(low, high, section, z, rise);
            z += rise;
            (low, high) = (high, low);
            section = BraceDebugSection::Repeating;
        }
    }
}

/// A preset's chain per pair (#360): links from `start_z` up, each rising
/// by the pair's span (at least the zigzag floor) with the preset's gap
/// above the last, in the bottom, middle or top pattern by its place; then
/// a ring at every reinforcement height, with the diagonal above it.
fn preset_chains(
    writer: &mut BraceWriter<'_>,
    pairs: &[Edge],
    preset: &StackedBracing,
    start_z: f64,
) {
    let shafts = writer.shafts;
    let diameter = writer.settings.brace_diameter_mm;
    for (index, edge) in pairs.iter().enumerate() {
        let (a, b) = (&shafts[edge.a], &shafts[edge.b]);
        // Shafts must stand a brace apart: between two that touch a brace
        // is a stub inside them, its ends in the open.
        let gap = edge.h_dist - (a.width() + b.width()) / 2.0;
        if !(gap >= diameter) {
            continue;
        }
        let rise = js_max(edge.h_dist, auto_bracing_hard_rules::MIN_ZIG_ZAG_RISE_MM);
        let step = rise + preset.gap_mm;
        let top = js_min(a.top_z(), b.top_z()) - 0.1;
        if top <= start_z + rise {
            continue;
        }
        let links = (((top - start_z - rise) / step).floor() + 1.0).max(0.0) as u32;
        let (low, high) = leaning(edge, index);
        for k in 0..links {
            let z = start_z + f64::from(k) * step;
            let section = section_of(k == 0);
            writer.place(low, high, section, z, rise);
            if preset.pattern_of(k, links) == AutoBracingPattern::CrossDiagonal {
                writer.place(high, low, section, z, rise);
            }
        }
        let repeat = BraceDebugSection::Repeating;
        for &h in &preset.reinforcement_heights_mm {
            // A ring is a cross at its height (a level brace would begin in
            // mid-air), plus its diagonal one link up.
            writer.place(low, high, repeat, h, rise);
            writer.place(high, low, repeat, h, rise);
            let above = h + step;
            match preset.reinforcement_diagonal {
                GridDiagonal::None => {}
                GridDiagonal::Single => writer.place(low, high, repeat, above, rise),
                GridDiagonal::Cross => {
                    writer.place(low, high, repeat, above, rise);
                    writer.place(high, low, repeat, above, rise);
                }
            }
        }
    }
}

/// The braces up one model's `pairs`, `max_z` its tallest shaft's top: the
/// preset's chains, or else a zigzag (when a pattern is one) and a tier of
/// the pattern at the first height and every interval above it.
pub(super) fn lay_ladder(writer: &mut BraceWriter<'_>, pairs: &[Edge], max_z: f64) {
    let settings = writer.settings;
    if let Some(preset) = &settings.stacked {
        preset_chains(writer, pairs, preset, settings.initial_distance_mm);
        return;
    }
    let first = settings.initial_distance_mm;
    let interval = settings.pattern_interval_mm;
    if settings.initial_pattern == AutoBracingPattern::ZigZag {
        zigzag(writer, pairs, first, max_z, BraceDebugSection::Initial);
    } else if settings.repeating_pattern == AutoBracingPattern::ZigZag {
        zigzag(
            writer,
            pairs,
            first + interval,
            max_z,
            BraceDebugSection::Repeating,
        );
    }
    let mut heights = vec![first];
    let mut z = first + interval;
    while z <= max_z {
        heights.push(z);
        z += interval;
    }
    for (tier, &z) in heights.iter().enumerate() {
        let pattern = if tier == 0 {
            settings.initial_pattern
        } else {
            settings.repeating_pattern
        };
        if pattern == AutoBracingPattern::ZigZag {
            continue;
        }
        let section = section_of(tier == 0);
        for edge in pairs {
            writer.place(edge.a, edge.b, section, z, 0.0);
            if pattern == AutoBracingPattern::CrossDiagonal {
                writer.place(edge.b, edge.a, section, z, 0.0);
            }
        }
    }
}

/// How far a shaft may stand above its highest brace before it is braced
/// higher up (#464).
const UNBRACED_TOP_MM: f64 = 10.0;

/// The plan-view box a shaft's segments span: min x, min y, max x, max y.
fn plan_box(shaft: &Shaft) -> [f64; 4] {
    shaft.pieces.iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |r, p| {
            [
                r[0].min(p.start.x).min(p.end.x),
                r[1].min(p.start.y).min(p.end.y),
                r[2].max(p.start.x).max(p.end.x),
                r[3].max(p.start.y).max(p.end.y),
            ]
        },
    )
}

/// The plan-view gap between two boxes of [`plan_box`].
fn box_gap(p: [f64; 4], q: [f64; 4]) -> f64 {
    let dx = js_max(0.0, js_max(q[0] - p[2], p[0] - q[2]));
    let dy = js_max(0.0, js_max(q[1] - p[3], p[1] - q[3]));
    (dx * dx + dy * dy).sqrt()
}

/// Braces the tops of tall trunks and branches (#464). Of the first `count`
/// shafts, each trunk or branch standing more than [`UNBRACED_TOP_MM`] above
/// its highest brace (or its foot, unbraced) is paired with the trunks and
/// branches of its model that rise beside it through more than that much of
/// the bare part, measured at the middle of the heights both span there: the
/// nearest, and the one nearest square to it. Each pair climbs from the
/// bare part's foot by the preset's chain, without rings. `held` are the
/// knots of braces kept from before the run; the braces laid for earlier
/// shafts count too.
pub(super) fn brace_tall_shafts<'k>(
    writer: &mut BraceWriter<'_>,
    count: usize,
    held: impl Iterator<Item = &'k Knot>,
) {
    let shafts = writer.shafts;
    let settings = writer.settings;
    let is_post = |i: usize| matches!(shafts[i].kind, SupportTypeId::Trunk | SupportTypeId::Branch);
    let owner: HashMap<&str, usize> = shafts[..count]
        .iter()
        .enumerate()
        .flat_map(|(i, s)| s.pieces.iter().map(move |p| (p.segment.id.as_str(), i)))
        .collect();
    let mut highest = vec![f64::NEG_INFINITY; count];
    let note = |knot: &Knot, highest: &mut [f64]| {
        if let Some(&i) = owner.get(knot.parent_shaft_id.as_str()) {
            highest[i] = js_max(highest[i], knot.pos.z);
        }
    };
    for knot in held {
        note(knot, &mut highest);
    }
    let boxes: Vec<[f64; 4]> = shafts[..count].iter().map(plan_box).collect();
    let preset = StackedBracing {
        reinforcement_heights_mm: Vec::new(),
        ..settings.stacked.clone().unwrap_or_default()
    };
    let min_rise = auto_bracing_hard_rules::MIN_ZIG_ZAG_RISE_MM;
    let min_span = min_pair_span_mm(settings);
    let max_len = settings.max_brace_length_mm;
    let mut noted = 0;
    for s in 0..count {
        for knot in writer.knots.values().skip(noted) {
            note(knot, &mut highest);
        }
        noted = writer.knots.len();
        if !is_post(s) {
            continue;
        }
        let me = &shafts[s];
        let braced_to = if highest[s].is_finite() {
            highest[s]
        } else {
            me.bottom_z()
        };
        let bare_from = js_max(braced_to, settings.initial_distance_mm);
        if me.top_z() - bare_from <= UNBRACED_TOP_MM {
            continue;
        }
        let mut beside: Vec<(Edge, f64)> = Vec::new();
        for (t, other) in shafts[..count].iter().enumerate() {
            if t == s
                || !is_post(t)
                || other.model_id != me.model_id
                || box_gap(boxes[s], boxes[t]) > max_len
            {
                continue;
            }
            let low = js_max(bare_from, other.bottom_z());
            let high = js_min(me.top_z(), other.top_z()) - 0.1;
            if high - low <= UNBRACED_TOP_MM {
                continue;
            }
            let Some((h_dist, angle_rad)) = span_at(me, other, (low + high) / 2.0) else {
                continue;
            };
            if h_dist < min_span || h_dist > max_len || high <= low + js_max(h_dist, min_rise) {
                continue;
            }
            let edge = Edge {
                a: s,
                b: t,
                h_dist,
                angle_rad,
            };
            beside.push((edge, low));
        }
        beside.sort_by(|x, y| js_compare(x.0.h_dist - y.0.h_dist));
        let Some(&nearest) = beside.first() else {
            continue;
        };
        let off_square =
            |e: &Edge| (90.0 - axis_separation_deg(nearest.0.angle_rad, e.angle_rad)).abs();
        let square = beside
            .iter()
            .filter(|(e, _)| {
                axis_separation_deg(nearest.0.angle_rad, e.angle_rad)
                    >= auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG
            })
            .min_by(|(x, _), (y, _)| js_compare(off_square(x) - off_square(y)))
            .copied();
        for (edge, low) in std::iter::once(nearest).chain(square) {
            preset_chains(writer, &[edge], &preset, low);
        }
    }
}

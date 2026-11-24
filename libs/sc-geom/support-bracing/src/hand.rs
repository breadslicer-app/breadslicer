//! Bracing by hand (#3): a shaft the person adds is braced to its
//! neighbours, and a brace the person draws between two shafts is laid by
//! the same rules and sizes a bracing run uses.

use std::collections::HashSet;

use super::ladder::{BraceWriter, lay_ladder};
use super::pairing::{Edge, bracing_grid, is_cardinal_delta, min_pair_span_mm, plan_point};
use super::samples::{EPS, Shaft, anchor_at, collect_shafts, plan_distance};
use bread_support_geom::geom::math_utils::{js_atan2, js_max, js_min};
use bread_support_model::auto_bracing::mesh_clearance::{BraceMeshes, line_clear_of_meshes};
use bread_support_model::auto_bracing::settings::{
    AutoBracingSettings, auto_bracing_hard_rules, normalize_auto_bracing_settings,
};
use bread_support_model::auto_bracing::two_axis_detection::{
    axis_separation_deg, normalize_axis_angle_rad,
};
use bread_support_model::constants::JOINT_DIAMETER_OFFSET_MM;
use bread_support_model::settings::types::SupportSettings;
use bread_support_model::support_kinds::is_auto_braceable_shaft_type;
use bread_support_model::types::{
    Brace, BraceDebugSection, Knot, SupportCollections, SupportState,
};
use bread_support_placement::auto_support::types::js_compare;

/// The shafts of `c` a brace may join.
fn braceable_shafts(c: &SupportCollections) -> Vec<Shaft> {
    collect_shafts(c, c)
        .into_iter()
        .filter(|s| is_auto_braceable_shaft_type(s.kind))
        .collect()
}

/// Braces the shaft `shaft_id` to its neighbours as a bracing run would
/// pair it, changing nothing else: to the nearest shaft of its model within
/// reach, and to the one nearest square to that for a second axis. The
/// braces go up by the run's ladder (the preset's chains, or its tiers and
/// zigzags), each within length, clear of the model. Returns how many
/// braces were added to `state`.
pub fn brace_new_shaft(
    state: &mut SupportState,
    shaft_id: &str,
    input_settings: &AutoBracingSettings,
    live: &SupportSettings,
    meshes: &BraceMeshes<'_>,
) -> usize {
    let settings = normalize_auto_bracing_settings(input_settings);
    let grid = bracing_grid(live);
    let c = &state.collections;
    let shafts = braceable_shafts(c);
    let Some(me) = shafts.iter().position(|s| s.id == shaft_id) else {
        return 0;
    };
    let model_id = shafts[me].model_id.as_str();
    let shortest = js_max(0.001, min_pair_span_mm(&settings));
    let here = plan_point(&shafts[me], grid);
    let mut reach: Vec<Edge> = Vec::new();
    for (i, other) in shafts.iter().enumerate() {
        if i == me || other.model_id != model_id {
            continue;
        }
        let there = plan_point(other, grid);
        let (dx, dy) = (there.x - here.x, there.y - here.y);
        if grid.enabled && !is_cardinal_delta(dx, dy, grid.spacing_mm) {
            continue;
        }
        let h_dist = (dx * dx + dy * dy).sqrt();
        if h_dist >= shortest && h_dist <= settings.max_brace_length_mm {
            reach.push(Edge {
                a: me,
                b: i,
                h_dist,
                angle_rad: normalize_axis_angle_rad(js_atan2(dy, dx)),
            });
        }
    }
    reach.sort_by(|x, y| js_compare(x.h_dist - y.h_dist));
    let Some((&nearest, rest)) = reach.split_first() else {
        return 0;
    };
    let mut square: Option<(Edge, f64)> = None;
    for &e in rest {
        let sep = axis_separation_deg(nearest.angle_rad, e.angle_rad);
        if sep < auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG {
            continue;
        }
        let score = 90.0 - (90.0 - sep).abs();
        if square.is_none_or(|(_, best)| score > best) {
            square = Some((e, score));
        }
    }
    let pairs: Vec<Edge> = std::iter::once(nearest)
        .chain(square.map(|(e, _)| e))
        .collect();
    let max_z = shafts
        .iter()
        .filter(|s| s.model_id == model_id)
        .fold(f64::NEG_INFINITY, |m, s| js_max(m, s.top_z()));
    let none = HashSet::new();
    let mut writer = BraceWriter::new(
        &shafts,
        &settings,
        grid,
        meshes,
        &none,
        c.braces.keys().cloned().collect(),
        c.knots.keys().cloned().collect(),
    );
    lay_ladder(&mut writer, &pairs, max_z);
    let (knots, braces) = (writer.knots, writer.braces);
    let added = braces.len();
    state.collections.knots.extend(knots);
    state.collections.braces.extend(braces);
    added
}

/// A brace a shaft had before it moved (#377): the shaft at its other end,
/// the heights of its two ends, and which end was the moved shaft's.
#[derive(Debug, Clone, PartialEq)]
pub struct LostBrace {
    /// The shaft at its other end.
    pub other: String,
    /// The height of its end on the moved shaft, mm.
    pub own_z: f64,
    /// The height of its end on the other shaft, mm.
    pub other_z: f64,
    /// Its start (the end a run lays first, the lower) was on the moved
    /// shaft.
    pub own_start: bool,
    pub section: BraceDebugSection,
}

/// Lays again on shaft `shaft_id` the braces it lost when it moved (#377):
/// each between the same two shafts at the same heights, at the bracing
/// settings' diameter, within their length and clear of the model as a
/// run's. One whose end no longer lies on its shaft is left out. Returns how
/// many were laid.
pub fn brace_again(
    state: &mut SupportState,
    shaft_id: &str,
    lost: &[LostBrace],
    input_settings: &AutoBracingSettings,
    live: &SupportSettings,
    meshes: &BraceMeshes<'_>,
) -> usize {
    let settings = normalize_auto_bracing_settings(input_settings);
    let grid = bracing_grid(live);
    let c = &state.collections;
    let shafts = braceable_shafts(c);
    let find = |id: &str| shafts.iter().position(|s| s.id == id);
    let Some(me) = find(shaft_id) else {
        return 0;
    };
    let none = HashSet::new();
    let mut writer = BraceWriter::new(
        &shafts,
        &settings,
        grid,
        meshes,
        &none,
        c.braces.keys().cloned().collect(),
        c.knots.keys().cloned().collect(),
    );
    let diameter = settings.brace_diameter_mm;
    for brace in lost {
        let Some(other) = find(&brace.other) else {
            continue;
        };
        let (Some(own), Some(far)) = (
            anchor_at(&shafts[me], brace.own_z),
            anchor_at(&shafts[other], brace.other_z),
        ) else {
            continue;
        };
        if plan_distance(own.pos, far.pos) > settings.max_brace_length_mm + EPS {
            continue;
        }
        if !line_clear_of_meshes(own.pos, far.pos, meshes, diameter) {
            continue;
        }
        if brace.own_start {
            writer.commit(me, other, brace.section, own, far);
        } else {
            writer.commit(other, me, brace.section, far, own);
        }
    }
    let (knots, braces) = (writer.knots, writer.braces);
    let added = braces.len();
    state.collections.knots.extend(knots);
    state.collections.braces.extend(braces);
    added
}

/// Why a brace drawn by hand between two shafts cannot go there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandBraceRefusal {
    /// Both ends are on the same shaft.
    SameSupport,
    /// The shafts hold different models.
    OtherModel,
    /// An end is on nothing a brace can join (a leaf, a twig, empty space).
    NotAShaft,
    /// An end is off its shaft: above where its tip begins, or below where
    /// a brace may start.
    OffShaft,
    /// The shafts are too close together for a brace.
    TooClose,
    /// Longer than the bracing settings allow.
    TooLong,
    /// A 45° brace from the first end would end off the second shaft; no
    /// brace is laid flatter than that.
    TooFlat,
    /// The brace would cross the model.
    HitsModel,
}

/// A brace drawn by hand and its knots: the first on the shaft it starts
/// from, the second on the other.
#[derive(Debug, Clone, PartialEq)]
pub struct HandBrace {
    pub brace: Brace,
    pub knots: [Knot; 2],
}

/// What [`hand_brace`] makes of two ends: the brace, and why it cannot go
/// there if it cannot (the brace is still given when both ends lie on their
/// shafts, to be drawn as refused).
#[derive(Debug, Clone, PartialEq)]
pub struct HandBraceOutcome {
    pub brace: Option<HandBrace>,
    pub refusal: Option<HandBraceRefusal>,
}

/// A brace drawn from shaft `from.0` at height `from.1` to shaft `to.0`
/// near height `to.1` (mm above the plate), sized and checked as a bracing
/// run's: its ends on the shafts' axes, no flatter than 45° (a second end
/// asked flatter moves along its shaft to 45°, towards the side it was
/// asked), within the length limit, clear of the model, at the settings'
/// diameter. Ids come from `new_id`; the brace belongs to the person
/// (`generated_by` unset), so a run's "remove existing" leaves it alone.
/// `state` is left unchanged.
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
    let shafts = braceable_shafts(c);
    let find = |id: &str| shafts.iter().position(|s| s.id == id);
    let (Some(a), Some(b)) = (find(from.0), find(to.0)) else {
        return refused(HandBraceRefusal::NotAShaft);
    };
    let (sa, sb) = (&shafts[a], &shafts[b]);
    if sa.model_id != sb.model_id {
        return refused(HandBraceRefusal::OtherModel);
    }
    // An end lies below where its shaft's tip begins and, for a preset, a
    // diameter above where the shaft starts: where a run keeps its ends.
    let lift = if settings.stacked.is_some() {
        settings.brace_diameter_mm
    } else {
        0.0
    };
    let floor = |s: &Shaft| s.bottom_z() + lift;
    let ceiling = |s: &Shaft| s.top_z() - 0.1;
    let on = |s: &Shaft, z: f64| z >= floor(s) - EPS && z <= ceiling(s) + EPS;
    let onto = |s: &Shaft, z: f64| js_max(floor(s), js_min(ceiling(s), z));
    let mut why: Option<HandBraceRefusal> = None;
    let refuse = |reason: HandBraceRefusal, why: &mut Option<HandBraceRefusal>| {
        why.get_or_insert(reason);
    };
    if !on(sa, from.1) || !on(sb, to.1) {
        refuse(HandBraceRefusal::OffShaft, &mut why);
    }
    let Some(start) = anchor_at(sa, onto(sa, from.1)) else {
        return refused(HandBraceRefusal::OffShaft);
    };
    // Shafts farther apart than a brace may run: said first, since no
    // height on either brings them closer.
    if let Some(there) = anchor_at(sb, onto(sb, to.1))
        && plan_distance(start.pos, there.pos) > settings.max_brace_length_mm + EPS
    {
        refuse(HandBraceRefusal::TooLong, &mut why);
    }
    // The second end: where asked, or at 45° when that is flatter, settled
    // over a few rounds as the ladder does (the shaft may lean).
    let up = if to.1 >= start.pos.z { 1.0 } else { -1.0 };
    let mut z = onto(sb, to.1);
    let mut moved = false;
    for _ in 0..4 {
        let Some(there) = anchor_at(sb, z) else {
            break;
        };
        let run = plan_distance(start.pos, there.pos);
        let wanted = if (to.1 - start.pos.z).abs() + EPS >= run {
            to.1
        } else {
            moved = true;
            start.pos.z + up * run
        };
        if !on(sb, wanted) {
            let reason = if moved {
                HandBraceRefusal::TooFlat
            } else {
                HandBraceRefusal::OffShaft
            };
            refuse(reason, &mut why);
        }
        let next = onto(sb, wanted);
        let settled = (next - z).abs() < 0.001;
        z = next;
        if settled {
            break;
        }
    }
    let Some(end) = anchor_at(sb, z) else {
        return refused(why.unwrap_or(HandBraceRefusal::OffShaft));
    };
    let span = plan_distance(start.pos, end.pos);
    let rise = (end.pos.z - start.pos.z).abs();
    // Still flatter than 45° once kept on the shaft: the 45° end is off it.
    if rise + 0.01 < span {
        refuse(HandBraceRefusal::TooFlat, &mut why);
    }
    if span < js_max(0.001, min_pair_span_mm(&settings)) {
        refuse(HandBraceRefusal::TooClose, &mut why);
    }
    let max_len = settings.max_brace_length_mm;
    if span > max_len + EPS || rise > max_len + EPS {
        refuse(HandBraceRefusal::TooLong, &mut why);
    }
    let diameter = settings.brace_diameter_mm;
    if !line_clear_of_meshes(start.pos, end.pos, meshes, diameter) {
        refuse(HandBraceRefusal::HitsModel, &mut why);
    }
    // A preset's diameter, capped by the thinner shaft, as a run sizes it.
    let preset_width = settings.stacked.is_some().then(|| {
        diameter
            .min(start.host_diameter_mm)
            .min(end.host_diameter_mm)
    });
    // The run's own writer, without the preset's height rule (the ends were
    // kept on their shafts above, and it would drop the brace silently).
    let plain = AutoBracingSettings {
        stacked: None,
        ..settings.clone()
    };
    let none = HashSet::new();
    let mut writer = BraceWriter::new(
        &shafts,
        &plain,
        bracing_grid(live),
        meshes,
        &none,
        c.braces.keys().cloned().collect(),
        c.knots.keys().cloned().collect(),
    );
    writer.commit(a, b, BraceDebugSection::Initial, start, end);
    let Some((_, mut brace)) = writer.braces.pop() else {
        return refused(why.unwrap_or(HandBraceRefusal::OffShaft));
    };
    let mut knots = writer.knots.into_values();
    let (Some(mut first), Some(mut second)) = (knots.next(), knots.next()) else {
        return refused(why.unwrap_or(HandBraceRefusal::OffShaft));
    };
    if let Some(width) = preset_width {
        first.diameter = Some(width + JOINT_DIAMETER_OFFSET_MM);
        second.diameter = Some(width + JOINT_DIAMETER_OFFSET_MM);
    }
    first.id = new_id();
    second.id = new_id();
    brace.base.id = new_id();
    brace.base.generated_by = None;
    brace.start_knot_id = first.id.clone();
    brace.end_knot_id = second.id.clone();
    HandBraceOutcome {
        brace: Some(HandBrace {
            brace,
            knots: [first, second],
        }),
        refusal: why,
    }
}

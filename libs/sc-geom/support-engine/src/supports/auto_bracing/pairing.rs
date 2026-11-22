//! Which shafts are braced to which. Pairs are picked in the plan view: the
//! shortest links that join every shaft of a model (a minimum spanning
//! tree), then for each shaft still held along a single axis one more link
//! as near square to it as there is.

use std::collections::{HashMap, HashSet};

use super::grouping::Vec2;
use super::samples::Shaft;
use super::settings::{AutoBracingSettings, auto_bracing_hard_rules};
use super::two_axis_detection::{
    axis_separation_deg, has_qualified_two_axis_bracing, normalize_axis_angle_rad,
};
use crate::geom::math_utils::{js_atan2, js_max};
use crate::supports::auto_support::types::js_compare;
use crate::supports::placement_logic::grid::grid_math::snap_to_grid_index;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_types::kickstand::kickstand_stabiliser::GridSnap;

/// Two shafts to brace (indices into the run's shafts), with the horizontal
/// distance and the axis between them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Edge {
    pub a: usize,
    pub b: usize,
    pub h_dist: f64,
    pub angle_rad: f64,
}

/// The grid braces follow: the support grid, unless it only places pillars.
pub(super) fn bracing_grid(live: &SupportSettings) -> GridSnap {
    GridSnap {
        enabled: live.grid.enabled && !live.grid.pillars,
        spacing_mm: live.grid.spacing_mm,
    }
}

/// How far off a grid line still counts as on it.
fn grid_tolerance(spacing_mm: f64) -> f64 {
    js_max(0.1, spacing_mm * 0.05)
}

/// True when the step (`dx`, `dy`) runs along a grid line.
pub(super) fn is_cardinal_delta(dx: f64, dy: f64, spacing_mm: f64) -> bool {
    let tol = grid_tolerance(spacing_mm);
    dx.abs() <= tol || dy.abs() <= tol
}

/// A shaft's foot in the plan view, snapped to the grid when braces follow
/// one.
pub(super) fn plan_point(shaft: &Shaft, grid: GridSnap) -> Vec2 {
    let foot = shaft.foot;
    if grid.enabled && !(grid.spacing_mm <= 0.0) {
        Vec2 {
            x: snap_to_grid_index(foot.x, grid.spacing_mm) * grid.spacing_mm,
            y: snap_to_grid_index(foot.y, grid.spacing_mm) * grid.spacing_mm,
        }
    } else {
        Vec2 {
            x: foot.x,
            y: foot.y,
        }
    }
}

/// Shafts closer than this are one post, not a pair.
pub(super) fn min_pair_span_mm(settings: &AutoBracingSettings) -> f64 {
    js_max(
        auto_bracing_hard_rules::MIN_PAIR_SPAN_MM,
        settings.brace_diameter_mm,
    )
}

/// The ids of a pair, the lower (in UTF-16 code-unit order) first.
pub(super) fn pair_key(a: &str, b: &str) -> (String, String) {
    let (low, high) = if a.encode_utf16().cmp(b.encode_utf16()).is_le() {
        (a, b)
    } else {
        (b, a)
    };
    (low.to_owned(), high.to_owned())
}

/// The other shaft of `e`, seen from `id`.
fn far_end<'s>(shafts: &'s [Shaft], e: &Edge, id: &str) -> &'s str {
    if shafts[e.a].id == id {
        &shafts[e.b].id
    } else {
        &shafts[e.a].id
    }
}

/// The pairs of the shafts `members` (one model's): every link between two
/// of them from the pair span to `max_len` long (along a grid line, when
/// braces follow one), shortest first; of those the minimum spanning tree,
/// then for each member short of two axes the link nearest square to one it
/// has, skipping links to a shaft that already shares a neighbour with it.
pub(super) fn model_pairs(
    shafts: &[Shaft],
    members: &[usize],
    max_len: f64,
    grid: GridSnap,
    min_span_mm: f64,
) -> Vec<Edge> {
    if members.len() < 2 {
        return Vec::new();
    }
    let tol = grid_tolerance(grid.spacing_mm);
    let shortest = js_max(0.001, min_span_mm);
    let points: Vec<Vec2> = members
        .iter()
        .map(|&s| plan_point(&shafts[s], grid))
        .collect();
    let mut links: Vec<Edge> = Vec::new();
    for (i, &p) in points.iter().enumerate() {
        for (j, &q) in points.iter().enumerate().skip(i + 1) {
            let (dx, dy) = (q.x - p.x, q.y - p.y);
            if grid.enabled && dx.abs() > tol && dy.abs() > tol {
                continue;
            }
            let h_dist = (dx * dx + dy * dy).sqrt();
            if h_dist < shortest || h_dist > max_len {
                continue;
            }
            links.push(Edge {
                a: members[i],
                b: members[j],
                h_dist,
                angle_rad: normalize_axis_angle_rad(js_atan2(dy, dx)),
            });
        }
    }
    links.sort_by(|x, y| js_compare(x.h_dist - y.h_dist));

    let id = |s: usize| shafts[s].id.as_str();
    let mut links_of: HashMap<&str, Vec<usize>> = HashMap::new();
    for (li, e) in links.iter().enumerate() {
        links_of.entry(id(e.a)).or_default().push(li);
        links_of.entry(id(e.b)).or_default().push(li);
    }
    let mut chosen: Vec<Edge> = Vec::new();
    let mut taken: HashSet<usize> = HashSet::new();
    let mut held: HashMap<&str, Vec<Edge>> = members.iter().map(|&s| (id(s), Vec::new())).collect();
    let take = |li: usize,
                chosen: &mut Vec<Edge>,
                taken: &mut HashSet<usize>,
                held: &mut HashMap<&str, Vec<Edge>>| {
        let e = links[li];
        chosen.push(e);
        taken.insert(li);
        for end in [e.a, e.b] {
            held.get_mut(id(end)).expect("a member").push(e);
        }
    };

    // The spanning tree, joining components shortest link first.
    let mut parent: HashMap<&str, &str> = members.iter().map(|&s| (id(s), id(s))).collect();
    for (li, e) in links.iter().enumerate() {
        let (ra, rb) = (root(&parent, id(e.a)), root(&parent, id(e.b)));
        if ra != rb {
            parent.insert(ra, rb);
            take(li, &mut chosen, &mut taken, &mut held);
        }
    }

    // A second axis for every member held along one.
    for &s in members {
        let me = id(s);
        let axes: Vec<f64> = held[me].iter().map(|e| e.angle_rad).collect();
        if has_qualified_two_axis_bracing(&axes, auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG) {
            continue;
        }
        let mine: HashSet<&str> = held[me].iter().map(|e| far_end(shafts, e, me)).collect();
        let mut best: Option<usize> = None;
        let mut best_score = -1.0;
        for &li in links_of.get(me).map_or(&[][..], Vec::as_slice) {
            if taken.contains(&li) {
                continue;
            }
            let e = links[li];
            let other = far_end(shafts, &e, me);
            if held[other]
                .iter()
                .any(|f| mine.contains(far_end(shafts, f, other)))
            {
                continue;
            }
            for &axis in &axes {
                let sep = axis_separation_deg(axis, e.angle_rad);
                if sep >= auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG {
                    let score = 90.0 - (90.0 - sep).abs();
                    if score > best_score {
                        best_score = score;
                        best = Some(li);
                    }
                }
            }
        }
        if let Some(li) = best {
            take(li, &mut chosen, &mut taken, &mut held);
        }
    }
    chosen
}

/// The representative of `x`'s component.
fn root<'s>(parent: &HashMap<&'s str, &'s str>, mut x: &'s str) -> &'s str {
    while parent[x] != x {
        x = parent[x];
    }
    x
}

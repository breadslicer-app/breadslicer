//! The route of a trunk from its tip cone's socket to the plate: one
//! diagonal at a fixed lean out of the way of the model, then a straight
//! drop. The search walks out along the diagonal in each of a fan of
//! directions until the column below clears, and keeps the joint nearest
//! the socket's own column (the more of the trunk is a plain pillar, the
//! stronger it is).
//!
//! At a given lateral offset the 45° point is the highest joint the lean
//! allows, so it ends the diagonal soonest; a steeper leg to the same column
//! is never better, and a flatter one leans off the model instead of
//! carrying it. A contact no walk serves gets no route.
//!
//! The work is bounded by the lateral reach, the height above the root and a
//! budget of distance-field probes, which the result reports.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use bread_support_geom::geom::math_utils::{
    js_atan2, js_cos, js_hypot, js_min, js_round, js_sin, js_tan,
};
use bread_support_model::collision_avoidance::CollisionField;
use bread_support_model::types::Vec3;

/// A horizontal direction or offset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Xy {
    pub x: f64,
    pub y: f64,
}

/// How [`find_escape_joint`] searches.
pub struct EscapeJointSearchOptions<'a> {
    /// The shaft's radius plus the clearance it keeps, mm.
    pub clearance_mm: f64,
    /// The step of the walk along the diagonal, mm.
    pub step_mm: f64,
    /// The furthest the joint may sit from the socket's column, mm.
    pub max_lateral_mm: f64,
    /// The diagonal's lean, degrees from vertical.
    pub lean_from_vertical_deg: f64,
    /// The shortest drop worth having below the joint, mm.
    pub min_vertical_leg_mm: f64,
    /// The directions to walk, unit, in order of preference (the order only
    /// breaks ties: a later walk still wins with a nearer joint).
    pub directions: Vec<Xy>,
    /// Whether the root's stack fits at a point.
    pub base_fits_at: &'a mut dyn FnMut(f64, f64) -> bool,
}

impl std::fmt::Debug for EscapeJointSearchOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EscapeJointSearchOptions")
            .field("clearance_mm", &self.clearance_mm)
            .field("step_mm", &self.step_mm)
            .field("max_lateral_mm", &self.max_lateral_mm)
            .finish_non_exhaustive()
    }
}

/// The joint a search found.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscapeJoint {
    /// Where the diagonal ends and the drop begins.
    pub joint: Vec3,
    /// The diagonal's lean, degrees from vertical.
    pub lean_from_vertical_deg: f64,
    /// How far the joint sits from the socket's column, mm.
    pub lateral_mm: f64,
    /// The direction it was found in; none from the grid search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction_index: Option<usize>,
}

/// How a search ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EscapeOutcome {
    Found,
    NoDirection,
    NeverCleared,
    ProbeBudget,
}

impl EscapeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            EscapeOutcome::Found => "found",
            EscapeOutcome::NoDirection => "no-direction",
            EscapeOutcome::NeverCleared => "never-cleared",
            EscapeOutcome::ProbeBudget => "probe-budget",
        }
    }
}

/// A search's joint, its cost in probes, and how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EscapeJointSearchResult {
    pub joint: Option<EscapeJoint>,
    pub probes: u32,
    pub outcome: EscapeOutcome,
}

impl EscapeJointSearchResult {
    fn none(probes: u32, outcome: EscapeOutcome) -> Self {
        EscapeJointSearchResult {
            joint: None,
            probes,
            outcome,
        }
    }

    fn of(best: Option<EscapeJoint>, probes: u32, otherwise: EscapeOutcome) -> Self {
        match best {
            Some(joint) => EscapeJointSearchResult {
                joint: Some(joint),
                probes,
                outcome: EscapeOutcome::Found,
            },
            None => Self::none(probes, otherwise),
        }
    }
}

/// The most distance-field probes one search may spend, with room over the
/// costliest search that found a joint.
pub const MAX_PROBES: u32 = 900;

const DEG: f64 = PI / 180.0;

/// The joint from which a drop to `root_top_z` clears the model and can
/// carry a root, walking out from `socket_pos`; none when nothing within
/// reach works.
pub fn find_escape_joint(
    sdf: &dyn CollisionField,
    socket_pos: Vec3,
    root_top_z: f64,
    opts: EscapeJointSearchOptions<'_>,
) -> EscapeJointSearchResult {
    let reach = js_min(
        opts.max_lateral_mm,
        socket_pos.z - root_top_z - opts.min_vertical_leg_mm,
    );
    if reach <= 0.0 || opts.directions.is_empty() {
        return EscapeJointSearchResult::none(0, EscapeOutcome::NoDirection);
    }
    prefetch_walks(sdf, socket_pos, root_top_z, reach, &opts);
    let found = walk(sdf, socket_pos, root_top_z, reach, opts);
    sdf.clear_prefetch();
    found
}

/// Steps per prefetch job: a long walk spreads over several threads.
const STEPS_PER_JOB: usize = 6;

/// Runs the walks ahead in parallel so that the real walk, which stays in
/// order and decides everything, finds most distances already worked out.
/// Each run of steps can start on its own (a step's points depend only on
/// the inputs) and stops where the real walk is expected to: where its walk
/// already ended, past a joint an earlier direction found, or once the
/// earlier walks and its own steps use up the budget. A wrong guess costs
/// only work; whether a root fits is left to the real walk.
fn prefetch_walks(
    sdf: &dyn CollisionField,
    socket: Vec3,
    root_top_z: f64,
    reach: f64,
    opts: &EscapeJointSearchOptions<'_>,
) {
    let dirs = &opts.directions;
    let clearance = opts.clearance_mm;
    let floor = root_top_z + opts.min_vertical_leg_mm;
    let (sin, cos) = (
        js_sin(opts.lean_from_vertical_deg * DEG),
        js_cos(opts.lean_from_vertical_deg * DEG),
    );
    let longest = reach / sin;
    // The distance along the diagonal at each step, summed as the walk sums it.
    let mut along = Vec::new();
    let mut d = opts.step_mm;
    while d <= longest && along.len() < MAX_PROBES as usize / 2 {
        along.push(d);
        d += opts.step_mm;
    }
    if along.is_empty() {
        return;
    }
    let point = |dir: Xy, k: usize| {
        let out = along[k] * sin;
        Vec3::new(
            socket.x + out * dir.x,
            socket.y + out * dir.y,
            socket.z - along[k] * cos,
        )
    };
    let n = dirs.len();
    // Per walk: the step it ended at, the steps passed without ending it, and
    // the lateral offset of its joint (positive f64 bits order as values do).
    let ended: Vec<AtomicUsize> = (0..n).map(|_| AtomicUsize::new(usize::MAX)).collect();
    let passed: Vec<AtomicUsize> = (0..n).map(|_| AtomicUsize::new(0)).collect();
    let joint_at: Vec<AtomicU64> = (0..n)
        .map(|_| AtomicU64::new(f64::INFINITY.to_bits()))
        .collect();
    // Nearest steps first, all directions together.
    sdf.prefetch(along.len().div_ceil(STEPS_PER_JOB) * n, &|job, field| {
        let (run, i) = (job / n, job % n);
        let first = run * STEPS_PER_JOB;
        let last = (first + STEPS_PER_JOB).min(along.len());
        for (k, &step) in along.iter().enumerate().take(last).skip(first) {
            if k > ended[i].load(Ordering::Relaxed) {
                return;
            }
            let out = step * sin;
            let mut spent = 2 * k;
            for j in 0..i {
                if out >= f64::from_bits(joint_at[j].load(Ordering::Relaxed)) {
                    return;
                }
                let end = ended[j].load(Ordering::Relaxed);
                spent += 2 * if end == usize::MAX {
                    passed[j].load(Ordering::Relaxed)
                } else {
                    end + 1
                };
            }
            if spent >= MAX_PROBES as usize {
                return;
            }
            let p = point(dirs[i], k);
            let previous = if k == 0 {
                socket
            } else {
                point(dirs[i], k - 1)
            };
            if p.z <= floor || field.segment_blocked(previous, p, clearance) {
                ended[i].fetch_min(k, Ordering::Relaxed);
                return;
            }
            if !field.segment_blocked(p, Vec3::new(p.x, p.y, root_top_z), clearance) {
                ended[i].fetch_min(k, Ordering::Relaxed);
                joint_at[i].fetch_min(out.to_bits(), Ordering::Relaxed);
                return;
            }
            passed[i].fetch_max(k + 1, Ordering::Relaxed);
        }
    });
}

/// The walks themselves, in order, each bounded by the best joint so far.
/// A step's diagonal must clear (or it may punch through a thin wall into
/// air beyond), then its column down to the root.
fn walk(
    sdf: &dyn CollisionField,
    socket: Vec3,
    root_top_z: f64,
    reach: f64,
    opts: EscapeJointSearchOptions<'_>,
) -> EscapeJointSearchResult {
    let lean = opts.lean_from_vertical_deg;
    let (sin, cos) = (js_sin(lean * DEG), js_cos(lean * DEG));
    let floor = root_top_z + opts.min_vertical_leg_mm;
    let mut probes = 0u32;
    let mut best: Option<EscapeJoint> = None;
    for (index, dir) in opts.directions.iter().enumerate() {
        let longest = best.map_or(reach, |b| b.lateral_mm) / sin;
        let mut previous = socket;
        let mut d = opts.step_mm;
        while d <= longest {
            if probes >= MAX_PROBES {
                return EscapeJointSearchResult::of(best, probes, EscapeOutcome::ProbeBudget);
            }
            let out = d * sin;
            if best.is_some_and(|b| out >= b.lateral_mm) {
                break;
            }
            let p = Vec3::new(
                socket.x + out * dir.x,
                socket.y + out * dir.y,
                socket.z - d * cos,
            );
            if p.z <= floor {
                break;
            }
            probes += 1;
            if sdf.segment_blocked(previous, p, opts.clearance_mm) {
                break;
            }
            probes += 1;
            let clear = !sdf.segment_blocked(p, Vec3::new(p.x, p.y, root_top_z), opts.clearance_mm);
            if clear && (opts.base_fits_at)(p.x, p.y) {
                best = Some(EscapeJoint {
                    joint: p,
                    lean_from_vertical_deg: lean,
                    lateral_mm: out,
                    direction_index: Some(index),
                });
                break;
            }
            previous = p;
            d += opts.step_mm;
        }
    }
    EscapeJointSearchResult::of(best, probes, EscapeOutcome::NeverCleared)
}

/// How [`find_grid_joint`] searches.
pub struct GridJointSearchOptions<'a> {
    /// The shaft's radius plus the clearance it keeps, mm.
    pub clearance_mm: f64,
    /// The grid's spacing: the joints tried stand on its nodes, mm.
    pub spacing_mm: f64,
    /// The furthest the joint may sit from the socket's column, mm.
    pub max_lateral_mm: f64,
    /// The steepest lean from vertical a run may take, degrees.
    pub lean_from_vertical_deg: f64,
    /// The shortest drop worth having below the joint, mm.
    pub min_vertical_leg_mm: f64,
    /// How many of the nearest nodes are tried at most.
    pub max_node_count: u32,
    /// The way the shaft already leaves (the cone axis seen from above):
    /// among nodes about as near, those closest to it go first. None for a
    /// flat ceiling.
    pub preferred_direction: Option<Xy>,
    /// Whether the root's stack fits at a point.
    pub base_fits_at: &'a mut dyn FnMut(f64, f64) -> bool,
    /// The leans to try, in order; empty tries only the steepest.
    pub leans_deg: &'a [f64],
}

impl std::fmt::Debug for GridJointSearchOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GridJointSearchOptions")
            .field("clearance_mm", &self.clearance_mm)
            .field("spacing_mm", &self.spacing_mm)
            .field("max_lateral_mm", &self.max_lateral_mm)
            .finish_non_exhaustive()
    }
}

/// The leans a run to a pillar may take, degrees from vertical, in order.
pub const PILLAR_LEANS_DEG: [f64; 4] = [45.0, 35.0, 25.0, 15.0];

/// The joint over a node of the grid from which a drop to `root_top_z`
/// clears and can carry a root: nodes in square rings round the socket's
/// own node, each ring by how close a node lies to the preferred direction;
/// at each node the leans in turn (a steeper run reaches the column lower
/// down, past what stood in the way).
pub fn find_grid_joint(
    sdf: &dyn CollisionField,
    socket: Vec3,
    root_top_z: f64,
    opts: GridJointSearchOptions<'_>,
) -> EscapeJointSearchResult {
    let spacing = opts.spacing_mm;
    let reach = js_min(
        opts.max_lateral_mm,
        socket.z - root_top_z - opts.min_vertical_leg_mm,
    );
    if reach <= 0.0 || spacing <= 0.0 {
        return EscapeJointSearchResult::none(0, EscapeOutcome::NoDirection);
    }
    // Nodes at multiples of the spacing, as grid keys are named.
    let origin = (
        js_round(socket.x / spacing) * spacing,
        js_round(socket.y / spacing) * spacing,
    );
    let own_lean = [opts.lean_from_vertical_deg];
    let leans = if opts.leans_deg.is_empty() {
        &own_lean[..]
    } else {
        opts.leans_deg
    };
    let mut probes = 0u32;
    // `None`: not this node, go on.
    let mut try_node = |ix: i64, iy: i64, probes: &mut u32| -> Option<EscapeJointSearchResult> {
        let (x, y) = (
            origin.0 + ix as f64 * spacing,
            origin.1 + iy as f64 * spacing,
        );
        let out = js_hypot(&[x - socket.x, y - socket.y]);
        if out > reach {
            return None;
        }
        if *probes >= MAX_PROBES {
            return Some(EscapeJointSearchResult::none(
                *probes,
                EscapeOutcome::ProbeBudget,
            ));
        }
        for &lean in leans {
            let lean = lean.min(opts.lean_from_vertical_deg);
            let z = socket.z - out / js_tan(lean * DEG);
            if z <= root_top_z + opts.min_vertical_leg_mm {
                continue;
            }
            let joint = Vec3::new(x, y, z);
            *probes += 1;
            if sdf.segment_blocked(socket, joint, opts.clearance_mm) {
                continue;
            }
            *probes += 1;
            if sdf.segment_blocked(joint, Vec3::new(x, y, root_top_z), opts.clearance_mm) {
                continue;
            }
            if !(opts.base_fits_at)(x, y) {
                return None;
            }
            return Some(EscapeJointSearchResult {
                joint: Some(EscapeJoint {
                    joint,
                    lean_from_vertical_deg: lean,
                    lateral_mm: out,
                    direction_index: None,
                }),
                probes: *probes,
                outcome: EscapeOutcome::Found,
            });
        }
        None
    };
    let preferred = opts.preferred_direction.map(|d| js_atan2(d.y, d.x));
    // A NaN reach walks no ring.
    let rings = (reach / spacing).ceil();
    let mut tried = 0u32;
    let mut ring = 0i64;
    while ring as f64 <= rings {
        let mut nodes: Vec<(i64, i64, f64)> = Vec::new();
        for ix in -ring..=ring {
            for iy in -ring..=ring {
                if ring > 0 && ix.abs() != ring && iy.abs() != ring {
                    continue;
                }
                let dx = origin.0 + ix as f64 * spacing - socket.x;
                let dy = origin.1 + iy as f64 * spacing - socket.y;
                let off = match preferred {
                    Some(p) if !(dx.abs() < 1e-9 && dy.abs() < 1e-9) => {
                        let turn = js_atan2(dy, dx) - p;
                        js_atan2(js_sin(turn), js_cos(turn)).abs()
                    }
                    _ => 0.0,
                };
                nodes.push((ix, iy, off));
            }
        }
        nodes.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        for (ix, iy, _) in nodes {
            if let Some(done) = try_node(ix, iy, &mut probes) {
                return done;
            }
            tried += 1;
            if tried >= opts.max_node_count {
                return EscapeJointSearchResult::none(probes, EscapeOutcome::ProbeBudget);
            }
        }
        ring += 1;
    }
    EscapeJointSearchResult::none(probes, EscapeOutcome::NeverCleared)
}

/// `count` directions round the compass: `preferred` first (or +X), then
/// its neighbours either side in turn.
pub fn build_direction_fan(preferred: Option<Xy>, count: usize) -> Vec<Xy> {
    let start = preferred.map_or(0.0, |p| js_atan2(p.y, p.x));
    (0..count)
        .map(|i| {
            let turn = if i == 0 {
                0.0
            } else {
                let side = if i % 2 == 1 { 1.0 } else { -1.0 };
                ((i as f64 / 2.0).ceil() * side * 2.0 * PI) / count as f64
            };
            let a = start + turn;
            Xy {
                x: js_cos(a),
                y: js_sin(a),
            }
        })
        .collect()
}

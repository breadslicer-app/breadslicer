//! DragonFruit `src/supports/PlacementLogicV3/EscapeJointSearch.ts`.
//!
//! EscapeJointSearch — the whole route search for a support trunk.
//!
//! A trunk has to get from the contact-cone socket to a point it can drop from
//! straight down. Slicers solve that with one diagonal and one joint, and this
//! searches for exactly that: in each direction, walk outward along a 45° ray
//! until the column below clears. The direction search is the whole search; it
//! is what minimises how far the joint has to sit from the socket's own column,
//! and a joint closer to vertical is a stronger support.
//!
//! Why 45° and not "as steep as it can get": for a given lateral offset the 45°
//! point is the highest joint the ceiling allows (drop >= lateral), so it ends
//! the diagonal sooner and starts the vertical earlier than any steeper leg to
//! the same column, and its diagonal is the shortest. Steeper is legal but never
//! better, so the search never spends probes on it.
//!
//! It never tilts the other way either: a contact the search cannot serve at
//! the shape is left unsupported and takes a pillar instead.
//!
//! What it deliberately is not:
//!
//! - Not a lattice search. No grid, no frontier, no cost function, no expansion
//!   budget, nothing to tune. Every candidate is tested against the signed
//!   distance field directly.
//! - Not a contour follower. The route never bends mid-air to hug the model: it
//!   leaves the socket once and drops once. Two segments, one joint.
//! - Not unbounded. The walk is capped by the lateral envelope, by the height
//!   available above the root, and by a hard probe budget, so a placement costs
//!   the same order of work whether the model is a cube or a skull.
//!
//! Cost is counted in SDF probes and reported back, so callers and tests can
//! hold it to a bound instead of trusting a comment.
//!
//! The field is the [`CollisionField`] the router holds (DragonFruit's
//! `SDFCache`); only `segmentBlocked` is asked.

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::types::Vec3;
use crate::three::math_utils::{js_atan2, js_cos, js_hypot, js_min, js_round, js_sin, js_tan};

/// A horizontal direction or offset, `{ x, y }`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Xy {
    pub x: f64,
    pub y: f64,
}

pub struct EscapeJointSearchOptions<'a> {
    /// Shaft keep-out radius: shaft radius + the global collision standoff.
    pub clearance_mm: f64,
    /// Lateral step of the outward walk, in mm.
    pub step_mm: f64,
    /// Hard cap on how far the joint may sit from the socket's own column.
    pub max_lateral_mm: f64,
    /// Lean of the diagonal, in degrees from vertical. The shape is one 45°
    /// diagonal out of the pocket, and it does not tilt further: a flatter leg
    /// is a member leaning off the model instead of carrying it.
    pub lean_from_vertical_deg: f64,
    /// Shortest vertical leg worth having below the joint, in mm.
    pub min_vertical_leg_mm: f64,
    /// Directions to try, as unit XY vectors, in preference order. The caller
    /// orders them: the surface normal's outward direction first, because that
    /// is the way off the surface the tip is attached to. Order decides ties
    /// only, since the search keeps looking for a joint closer to the socket's
    /// own column than the one it already has.
    pub directions: Vec<Xy>,
    /// True when the roots volume can sit at this XY.
    pub base_fits_at: &'a mut dyn FnMut(f64, f64) -> bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscapeJoint {
    /// Where the diagonal ends and the vertical leg begins.
    pub joint: Vec3,
    /// Lean of the diagonal, degrees from vertical. Never above the ceiling.
    pub lean_from_vertical_deg: f64,
    /// Lateral distance from the socket's column.
    pub lateral_mm: f64,
    /// Index in `options.directions` that produced this joint. Absent for the grid search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction_index: Option<usize>,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EscapeJointSearchResult {
    pub joint: Option<EscapeJoint>,
    /// SDF probes spent: the unit of cost this search is held to.
    pub probes: u32,
    pub outcome: EscapeOutcome,
}

/// Hard ceiling on SDF probes for one search.
///
/// DragonFruit briefly cut it to 600 on one run's histogram and the next run of
/// the same model needed 601, so it stays clear of the worst success and
/// `RouterStats::max_found_probes` reports what the margin actually is.
pub const MAX_PROBES: u32 = 900;

const DEG: f64 = PI / 180.0;

/// Finds the joint: the first point, in the best direction, from which a
/// straight drop to `root_top_z` is clear and can carry a root.
///
/// `socket_pos` is where the shaft leaves the contact cone. Returns no joint
/// when nothing inside the envelope works, which the caller treats as "no
/// route" rather than growing the search.
pub fn find_escape_joint(
    sdf: &dyn CollisionField,
    socket_pos: Vec3,
    root_top_z: f64,
    opts: EscapeJointSearchOptions<'_>,
) -> EscapeJointSearchResult {
    let mut probes = 0u32;
    let clearance = opts.clearance_mm;

    let max_lateral_mm = js_min(
        opts.max_lateral_mm,
        socket_pos.z - root_top_z - opts.min_vertical_leg_mm,
    );
    if max_lateral_mm <= 0.0 || opts.directions.is_empty() {
        return EscapeJointSearchResult {
            joint: None,
            probes,
            outcome: EscapeOutcome::NoDirection,
        };
    }

    // The objective is the lateral offset: everything below the joint is
    // vertical, so the closer the joint sits to the socket's own column, the
    // more of the support is a plain pillar. `directions` is therefore a
    // preference order for ties, not a decision.
    let mut best: Option<EscapeJoint> = None;

    for (direction_index, dir) in opts.directions.iter().enumerate() {
        let lean_deg = opts.lean_from_vertical_deg;
        let sin_lean = js_sin(lean_deg * DEG);
        let cos_lean = js_cos(lean_deg * DEG);
        // Once a joint is in hand, every remaining walk only has to beat it.
        let lateral_limit_mm = best.map_or(max_lateral_mm, |b| b.lateral_mm);
        let max_travel_mm = lateral_limit_mm / sin_lean;
        let mut previous = socket_pos;

        let mut travelled_mm = opts.step_mm;
        while travelled_mm <= max_travel_mm {
            if probes >= MAX_PROBES {
                return match best {
                    Some(joint) => EscapeJointSearchResult {
                        joint: Some(joint),
                        probes,
                        outcome: EscapeOutcome::Found,
                    },
                    None => EscapeJointSearchResult {
                        joint: None,
                        probes,
                        outcome: EscapeOutcome::ProbeBudget,
                    },
                };
            }

            let lateral_mm = travelled_mm * sin_lean;
            if best.is_some_and(|b| lateral_mm >= b.lateral_mm) {
                break;
            }
            let point = Vec3::new(
                socket_pos.x + lateral_mm * dir.x,
                socket_pos.y + lateral_mm * dir.y,
                socket_pos.z - travelled_mm * cos_lean,
            );
            if point.z <= root_top_z + opts.min_vertical_leg_mm {
                break;
            }

            // The leg itself must stay clear. Without this, a diagonal aimed
            // across the model can punch through a thin wall and emerge into
            // open air on the far side, which would read as a valid escape.
            probes += 1;
            if sdf.segment_blocked(previous, point, clearance) {
                break;
            }

            probes += 1;
            let column_clear =
                !sdf.segment_blocked(point, Vec3::new(point.x, point.y, root_top_z), clearance);
            if column_clear && (opts.base_fits_at)(point.x, point.y) {
                best = Some(EscapeJoint {
                    joint: point,
                    lean_from_vertical_deg: lean_deg,
                    lateral_mm,
                    direction_index: Some(direction_index),
                });
                // This walk cannot beat itself: lateral only grows along it.
                break;
            }

            previous = point;
            travelled_mm += opts.step_mm;
        }
    }

    match best {
        Some(joint) => EscapeJointSearchResult {
            joint: Some(joint),
            probes,
            outcome: EscapeOutcome::Found,
        },
        None => EscapeJointSearchResult {
            joint: None,
            probes,
            outcome: EscapeOutcome::NeverCleared,
        },
    }
}

pub struct GridJointSearchOptions<'a> {
    /// Shaft keep-out radius: shaft radius + the global collision standoff.
    pub clearance_mm: f64,
    /// Lattice spacing. The candidate joints are the nodes of this lattice.
    pub spacing_mm: f64,
    /// Hard cap on how far the joint may sit from the socket's own column.
    pub max_lateral_mm: f64,
    /// Lean from vertical, in degrees. See [`find_escape_joint`].
    pub lean_from_vertical_deg: f64,
    /// Shortest vertical leg worth having below the joint, in mm.
    pub min_vertical_leg_mm: f64,
    /// How many of the nearest nodes to try before giving up.
    pub max_node_count: u32,
    /// The direction the shaft is already travelling in, as the horizontal
    /// component of the cone axis. Among nodes at the same distance the ones
    /// nearest this direction are tried first, so the escape keeps leaving the
    /// way it started instead of hunting sideways for a node. None when the
    /// direction is degenerate (a flat ceiling), where any node is as good.
    pub preferred_direction: Option<Xy>,
    /// True when the roots volume can sit at this XY.
    pub base_fits_at: &'a mut dyn FnMut(f64, f64) -> bool,
}

/// The grid-mode search: the same gates as [`find_escape_joint`], but the
/// candidate joints are lattice nodes rather than free points along a ray.
///
/// Grid mode has to land the drop on a node, so it chooses the node first and
/// derives the joint from it: the drop is `lateral / tan(lean)`, which makes the
/// vertical leg exactly vertical by construction. Nodes are walked
/// nearest-first, so the joint stays as close to the socket's own column as the
/// model allows, and the first node that satisfies every gate wins.
pub fn find_grid_joint(
    sdf: &dyn CollisionField,
    socket_pos: Vec3,
    root_top_z: f64,
    opts: GridJointSearchOptions<'_>,
) -> EscapeJointSearchResult {
    let mut probes = 0u32;
    let clearance = opts.clearance_mm;
    let spacing = opts.spacing_mm;

    let max_lateral_mm = js_min(
        opts.max_lateral_mm,
        socket_pos.z - root_top_z - opts.min_vertical_leg_mm,
    );
    if max_lateral_mm <= 0.0 || spacing <= 0.0 {
        return EscapeJointSearchResult {
            joint: None,
            probes,
            outcome: EscapeOutcome::NoDirection,
        };
    }

    // The lattice is the one the grid keys are named on: multiples of the
    // spacing, so a node round-trips through `gridNodeKeyFromXY`.
    let origin_x = js_round(socket_pos.x / spacing) * spacing;
    let origin_y = js_round(socket_pos.y / spacing) * spacing;

    // Tests one node. `None` means "not this node, keep walking".
    let mut try_node = |ix: i64, iy: i64, probes: &mut u32| -> Option<EscapeJointSearchResult> {
        let x = origin_x + ix as f64 * spacing;
        let y = origin_y + iy as f64 * spacing;
        let lateral_mm = js_hypot(&[x - socket_pos.x, y - socket_pos.y]);
        if lateral_mm > max_lateral_mm {
            return None;
        }

        if *probes >= MAX_PROBES {
            return Some(EscapeJointSearchResult {
                joint: None,
                probes: *probes,
                outcome: EscapeOutcome::ProbeBudget,
            });
        }

        // The joint sits where the shape's ray from the socket passes this
        // node's column, so the drop follows from the lean.
        let lean_deg = opts.lean_from_vertical_deg;
        let drop_mm = lateral_mm / js_tan(lean_deg * DEG);
        let z = socket_pos.z - drop_mm;
        if z <= root_top_z + opts.min_vertical_leg_mm {
            return None;
        }

        let joint = Vec3::new(x, y, z);
        *probes += 1;
        if sdf.segment_blocked(socket_pos, joint, clearance) {
            return None;
        }
        *probes += 1;
        if sdf.segment_blocked(joint, Vec3::new(x, y, root_top_z), clearance) {
            return None;
        }
        if !(opts.base_fits_at)(x, y) {
            return None;
        }
        Some(EscapeJointSearchResult {
            joint: Some(EscapeJoint {
                joint,
                lean_from_vertical_deg: lean_deg,
                lateral_mm,
                direction_index: None,
            }),
            probes: *probes,
            outcome: EscapeOutcome::Found,
        })
    };

    // Square rings outwards from the socket's own node, so the nearest node that
    // works is the one that wins. Within a ring the nodes sit at similar
    // distances, so that is where direction is decided: nearest the direction
    // the shaft is already travelling first.
    let preferred_angle = opts.preferred_direction.map(|d| js_atan2(d.y, d.x));
    // A float bound, as in `ring <= maxRings`: NaN walks no ring at all.
    let max_rings = (max_lateral_mm / spacing).ceil();
    let mut tested = 0u32;
    let mut ring = 0i64;
    while ring as f64 <= max_rings {
        let mut offsets: Vec<(i64, i64, f64)> = Vec::new();
        for ix in -ring..=ring {
            for iy in -ring..=ring {
                if ring > 0 && ix.abs() != ring && iy.abs() != ring {
                    continue;
                }
                let dx = origin_x + ix as f64 * spacing - socket_pos.x;
                let dy = origin_y + iy as f64 * spacing - socket_pos.y;
                let deviation = match preferred_angle {
                    Some(preferred) if !(dx.abs() < 1e-9 && dy.abs() < 1e-9) => js_atan2(
                        js_sin(js_atan2(dy, dx) - preferred),
                        js_cos(js_atan2(dy, dx) - preferred),
                    )
                    .abs(),
                    _ => 0.0,
                };
                offsets.push((ix, iy, deviation));
            }
        }
        // `Array.prototype.sort` is stable, and so is this.
        offsets.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        for (ix, iy, _) in offsets {
            if let Some(attempt) = try_node(ix, iy, &mut probes) {
                return attempt;
            }
            tested += 1;
            if tested >= opts.max_node_count {
                return EscapeJointSearchResult {
                    joint: None,
                    probes,
                    outcome: EscapeOutcome::ProbeBudget,
                };
            }
        }
        ring += 1;
    }

    EscapeJointSearchResult {
        joint: None,
        probes,
        outcome: EscapeOutcome::NeverCleared,
    }
}

/// Directions to try, in preference order: the outward direction the caller
/// cares about first, then its neighbours around the compass, alternating
/// either side. A direction is only ever a tie-break between equally good
/// joints, so a coarse fan is enough.
pub fn build_direction_fan(preferred: Option<Xy>, count: usize) -> Vec<Xy> {
    let base_angle = preferred.map_or(0.0, |p| js_atan2(p.y, p.x));
    (0..count)
        .map(|i| {
            let offset = if i == 0 {
                0.0
            } else {
                let side = if i % 2 == 1 { 1.0 } else { -1.0 };
                ((i as f64 / 2.0).ceil() * side * 2.0 * PI) / count as f64
            };
            let angle = base_angle + offset;
            Xy {
                x: js_cos(angle),
                y: js_sin(angle),
            }
        })
        .collect()
}

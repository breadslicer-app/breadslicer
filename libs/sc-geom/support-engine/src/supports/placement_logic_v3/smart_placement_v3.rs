//! The trunk router: where a trunk's shaft runs from its tip cone to the
//! plate, in the one shape a printed support takes:
//!
//! ```text
//! tip cone ──► one diagonal ──► joint ──► straight drop to the plate
//! ```
//!
//! First a straight drop under the cone (the cone may swing round the tip to
//! find one); else one diagonal, at up to 45° from vertical, to the nearest
//! column that clears ([`super::escape_joint_search`]), then straight down.
//! A contact neither serves is refused with the model in the way; one the
//! grid cannot serve at the shape is routed again with the grid off; one the
//! model blocks gets a second, tighter look with the tip leaning further.
//!
//! The work is bounded: a capped search with a probe budget, and the root's
//! volume, are all that touch the model; nothing iterates to a fixed point.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::escape_joint_search::{
    EscapeJointSearchOptions, GridJointSearchOptions, PILLAR_LEANS_DEG, Xy, build_direction_fan,
    find_escape_joint, find_grid_joint,
};
use crate::geom::Matrix4;
use crate::geom::math_utils::{deg_to_rad, js_cos, js_hypot, js_max, js_min, js_round, js_sin};
use crate::supports::placement_logic::collision_avoidance::{
    CollisionField, ContactConeSweep, is_contact_cone_blocked,
};
use crate::supports::placement_logic::cone_axis_policy::{
    MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG, clamp_cone_axis_deviation_from_surface_normal,
    cone_keeps_contact_highest,
};
use crate::supports::placement_logic::grid::grid_math::{
    grid_node_key_from_xy, grid_snapped_xy_from_key,
};
use crate::supports::placement_logic::grid::nearest_candidate_node_keys::build_nearest_candidate_node_keys;
use crate::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use crate::supports::placement_logic::smart_placement_search_utils::{
    TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG, distance_3d, distance_xy,
    get_length_aware_max_angle_from_vertical_deg,
    segment_satisfies_length_aware_max_angle_from_vertical,
    segment_satisfies_max_angle_from_vertical, span_lean_from_vertical_deg,
};
use crate::supports::placement_logic::standard_placement::{
    TrunkPlacementInput, TrunkPlacementResult, calculate_standard_placement,
};
use crate::supports::settings::types::SupportSettings;
use crate::supports::tips::{SupportTipProfile, get_socket_position, nib_length};
use crate::supports::types::{LimitationCode, Vec3};

/// What the router asked the model, counted, for the run report: its cost
/// is the number of questions put to the distance field.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterStats {
    pub placements: u64,
    /// Cones tried for a straight drop or a routed start.
    pub cones_tested: u64,
    /// Cone checks that reached the distance field (not remembered).
    pub cone_gates: u64,
    /// Joint searches run.
    pub joint_searches: u64,
    /// Probes those searches spent.
    pub joint_probes: u64,
    /// Root volume checks not remembered for their spot.
    pub roots_checks: u64,
    /// Distance queries those checks made.
    pub roots_samples: u64,
    /// Spots tried for a root.
    pub base_candidates: u64,
    /// How the joint searches ended, in the order first seen.
    pub joint_outcomes: IndexMap<String, u64>,
    /// The probes of the searches that found a joint, by power of two (up
    /// to 64, 128, 256, 512, more).
    pub found_probe_buckets: [u64; 5],
    /// The most probes a search that found a joint spent.
    pub max_found_probes: u64,
}

impl RouterStats {
    pub fn reset(&mut self) {
        *self = RouterStats::default();
    }
}

/// A contact to route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmartPlacementV3Input<'a> {
    pub placement: TrunkPlacementInput,
    pub model_id: &'a str,
    /// A hover preview rather than a placement; nothing here reads it.
    pub is_preview: Option<bool>,
}

/// How a routing pass runs; set only by the router's own second passes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SmartPlacementV3Context {
    /// Route as if the grid were off.
    pub ignore_grid: bool,
    /// The tight pass (#466): only cones leaning past the normal cap.
    pub tight: bool,
}

/// The clearance shafts keep from the model past their radius, mm.
const STANDOFF_MM: f64 = 0.48;
/// The step a shaft's radius is rounded up to for its clearance (#233), mm.
pub const CLEARANCE_RADIUS_STEP_MM: f64 = 0.025;
/// The column map's cell, mm.
pub const COLUMN_MAP_CELL_MM: f64 = 0.2;
/// Root spots tried round the preferred one with the grid off (with it, the
/// joint already stands over a node: a root that does not fit there means
/// another joint).
const BASE_RINGS: u32 = 4;
const GRID_BASE_RINGS: u32 = 0;
/// The shortest drop worth having below a joint, mm.
const MIN_DROP_MM: f64 = 1.0;
/// A root this far off the socket's column is still a straight drop, mm.
const STRAIGHT_TOLERANCE_MM: f64 = 0.05;
/// The step of the walk along a diagonal, mm.
const WALK_STEP_MM: f64 = 0.5;
/// The directions walked.
const DIRECTION_COUNT: usize = 12;
/// Nodes the grid search tries, nearest first.
const GRID_NODES: u32 = 24;
/// How far from a tip a pillar may stand, in grid steps (#464).
const PILLAR_REACH_STEPS: f64 = 0.75;
/// Nodes tried with pillars on the grid (#464).
const PILLAR_NODES: u32 = 80;
/// The drop below the joint may lean this much past the routed angle
/// configured, degrees, and the routed angle is never under the floor.
const DROP_SLACK_DEG: f64 = 10.0;
const ROUTED_FLOOR_DEG: f64 = 15.0;
/// How far a blocked cone swings off the normal, degrees, smallest first,
/// and in how many directions round the tip.
const CONE_SWINGS_DEG: [f64; 3] = [10.0, 20.0, 30.0];
const CONE_SWING_DIRECTIONS: usize = 8;
/// The tight pass's swing (#466), degrees: on the scorecard 45 places most
/// of what the model was in the way of and buries no more tips.
const TIGHT_SWING_DEG: f64 = 45.0;
/// Samples round a root's cross-section at each height.
const ROOT_RIM_SAMPLES: u32 = 16;

/// The clearance a shaft of `shaft_diameter_mm` keeps: its radius rounded up
/// to [`CLEARANCE_RADIUS_STEP_MM`] (so shafts within 0.05 mm share a column
/// map), plus the standoff.
pub fn column_clearance_mm(shaft_diameter_mm: f64) -> f64 {
    let radius = shaft_diameter_mm / 2.0;
    // A hair under, so a radius on a step stays there.
    let steps = (radius / CLEARANCE_RADIUS_STEP_MM - 1e-9).ceil();
    steps * CLEARANCE_RADIUS_STEP_MM + STANDOFF_MM
}

/// A remembered answer's key part: a rounded value, with -0 and 0 one key
/// and every NaN one key.
fn memo_part(rounded: f64) -> u64 {
    if rounded == 0.0 {
        0.0f64.to_bits()
    } else if rounded.is_nan() {
        f64::NAN.to_bits()
    } else {
        rounded.to_bits()
    }
}

/// A cone the router may use: where it leaves the contact disk, its axis,
/// and the socket the shaft leaves from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cone {
    socket_pos: Vec3,
    cone_axis: Vec3,
    cone_start_pos: Vec3,
}

/// Where a root lands and where the drop above it ends.
#[derive(Debug, Clone, PartialEq)]
struct Base {
    base_pos: Vec3,
    root_top: Vec3,
    node_key: Option<String>,
}

/// One placement's checks against the model, with the costly ones
/// remembered (the segment check caches its cells already).
struct Checks<'a> {
    sdf: &'a SdfCache,
    stats: &'a mut RouterStats,
    tip: Vec3,
    profile: SupportTipProfile,
    clearance: f64,
    disk_height: f64,
    cone_height: f64,
    root_radius: f64,
    shaft_radius: f64,
    /// How far the cone may swing off the normal, degrees.
    max_swing_deg: f64,
    roots: HashMap<(u64, u64), bool>,
    cones: HashMap<[u64; 6], bool>,
}

impl Checks<'_> {
    /// Whether the root's disk and cone at (x, y) reach the model:
    /// remembered per 0.05 mm spot.
    fn root_blocked(&mut self, x: f64, y: f64) -> bool {
        let key = (memo_part(js_round(x / 0.05)), memo_part(js_round(y / 0.05)));
        if let Some(&known) = self.roots.get(&key) {
            return known;
        }
        let blocked = self.root_volume_blocked(x, y);
        self.roots.insert(key, blocked);
        blocked
    }

    /// The root's cross-section, slice by slice up its height: the centre's
    /// distance settles a slice when it clears the whole section (the field
    /// is 1-Lipschitz) or the standoff alone fails; else its rim is sampled.
    fn root_volume_blocked(&mut self, x: f64, y: f64) -> bool {
        self.stats.roots_checks += 1;
        let top = self.disk_height + self.cone_height;
        let slices = js_max(4.0, (top / self.sdf.cell_size()).ceil());
        let mut slice = 0.0;
        while slice <= slices {
            let z = (slice / slices) * top;
            slice += 1.0;
            let r = if z <= self.disk_height {
                self.root_radius
            } else {
                self.root_radius
                    + ((z - self.disk_height) / js_max(self.cone_height, 1e-6))
                        * (self.shaft_radius - self.root_radius)
            };
            self.stats.roots_samples += 1;
            let centre = self.sdf.distance_at(x, y, z);
            if centre >= r + STANDOFF_MM {
                continue;
            }
            if centre < STANDOFF_MM {
                return true;
            }
            for i in 0..ROOT_RIM_SAMPLES {
                let a = (f64::from(i) / f64::from(ROOT_RIM_SAMPLES)) * std::f64::consts::PI * 2.0;
                self.stats.roots_samples += 1;
                if self
                    .sdf
                    .distance_at(x + js_cos(a) * r, y + js_sin(a) * r, z)
                    < STANDOFF_MM
                {
                    return true;
                }
            }
        }
        false
    }

    fn segment_blocked(&self, from: Vec3, to: Vec3) -> bool {
        self.sdf
            .segment_blocked_xyz(from.x, from.y, from.z, to.x, to.y, to.z, self.clearance)
    }

    /// Whether the socket itself is within the clearance of the model.
    fn socket_blocked(&self, socket: Vec3) -> bool {
        self.sdf.distance_at(socket.x, socket.y, socket.z) < self.clearance
    }

    /// Whether the cone's body reaches the model (a cone can lean into what
    /// its socket clears), remembered per 0.01 mm.
    fn cone_blocked(&mut self, start: Vec3, socket: Vec3) -> bool {
        let key = [start.x, start.y, start.z, socket.x, socket.y, socket.z]
            .map(|v| memo_part(js_round(v * 100.0)));
        if let Some(&known) = self.cones.get(&key) {
            return known;
        }
        self.stats.cone_gates += 1;
        let blocked = is_contact_cone_blocked(
            self.sdf,
            ContactConeSweep {
                start,
                end: socket,
                start_radius: self.profile.contact_diameter_mm / 2.0,
                end_radius: self.profile.body_diameter_mm / 2.0,
            },
        );
        self.cones.insert(key, blocked);
        blocked
    }

    /// Whether the cone leaves its contact the highest point of the support.
    fn contact_on_top(&self, cone: &Cone) -> bool {
        cone_keeps_contact_highest(
            cone.cone_axis,
            cone.socket_pos,
            self.tip,
            self.profile.body_diameter_mm,
        )
    }

    /// The cone for a shaft starting at `socket` and running through
    /// `joints`: aimed at the first joint (within the swing allowed) when
    /// that lines the cone up with the shaft and clears; else the cone at
    /// `socket` as it is.
    fn cone_toward(
        &mut self,
        socket: Vec3,
        joints: &[Vec3],
        normal: Vec3,
        axis_hint: Vec3,
    ) -> Cone {
        let (tip, profile) = (self.tip, self.profile);
        let nib = if profile.is_disk() {
            nib_length(normal, axis_hint, Some(&profile.as_disk_profile()))
        } else {
            0.0
        };
        let start = Vec3::new(
            tip.x + normal.x * nib,
            tip.y + normal.y * nib,
            tip.z + normal.z * nib,
        );
        if let Some(&joint) = joints.first() {
            let axis = clamp_cone_axis_deviation_from_surface_normal(
                normal,
                Vec3::new(joint.x - start.x, joint.y - start.y, joint.z - start.z).normalize(),
                self.max_swing_deg,
            );
            let aimed = get_socket_position(start, axis, Some(&profile));
            if !self.socket_blocked(aimed)
                && !self.cone_blocked(start, aimed)
                && !self.segment_blocked(aimed, joint)
                && cone_keeps_contact_highest(axis, aimed, tip, profile.body_diameter_mm)
            {
                return Cone {
                    socket_pos: aimed,
                    cone_axis: axis,
                    cone_start_pos: start,
                };
            }
        }
        Cone {
            socket_pos: socket,
            cone_axis: Vec3::new(socket.x - start.x, socket.y - start.y, socket.z - start.z)
                .normalize(),
            cone_start_pos: start,
        }
    }

    /// Where the root lands for a drop from `drop_from`: at `preferred` with
    /// the grid off, else on the node nearest it within `rings` (the root
    /// fitting and the drop clearing), nearest under the joint first.
    fn base_for(
        &mut self,
        preferred: Xy,
        drop_from: Vec3,
        root_top_z: f64,
        spacing_mm: f64,
        grid: bool,
        rings: u32,
    ) -> Option<Base> {
        let keys = if grid {
            build_nearest_candidate_node_keys(
                &grid_node_key_from_xy(preferred.x, preferred.y, spacing_mm),
                rings,
            )
        } else {
            vec![String::new()]
        };
        let mut best: Option<Base> = None;
        let mut best_off = f64::INFINITY;
        for key in keys {
            self.stats.base_candidates += 1;
            let (x, y) = if grid {
                grid_snapped_xy_from_key(&key, spacing_mm)
            } else {
                (preferred.x, preferred.y)
            };
            if self.root_blocked(x, y) {
                continue;
            }
            let root_top = Vec3::new(x, y, root_top_z);
            if self.segment_blocked(drop_from, root_top) {
                continue;
            }
            let off = js_hypot(&[x - drop_from.x, y - drop_from.y]);
            if best.is_none() || off < best_off {
                best = Some(Base {
                    base_pos: Vec3::new(x, y, 0.0),
                    root_top,
                    node_key: grid.then_some(key),
                });
                best_off = off;
            }
        }
        best
    }
}

/// The axes a cone may swing to round what stands beside the tip: each of
/// `swings_deg` off the normal, in a fan round it starting the way off the
/// surface (`outward`).
fn swung_axes(normal: Vec3, outward: Vec3, swings_deg: &[f64]) -> Vec<Vec3> {
    if normal.length_sq() < 1e-10 {
        return Vec::new();
    }
    let normal = normal.normalize();
    let helper = if normal.z.abs() < 0.9 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let u = Vec3::cross_vectors(helper, normal).normalize();
    let v = Vec3::cross_vectors(normal, u).normalize();
    let flat = outward.add_scaled_vector(normal, -outward.dot(normal));
    let first = if flat.length_sq() > 1e-6 {
        Xy {
            x: flat.dot(u),
            y: flat.dot(v),
        }
    } else {
        Xy { x: 1.0, y: 0.0 }
    };
    let mut axes = Vec::new();
    for &swing in swings_deg {
        let angle = deg_to_rad(swing);
        for d in build_direction_fan(Some(first), CONE_SWING_DIRECTIONS) {
            let toward = u.multiply_scalar(d.x).add_scaled_vector(v, d.y);
            axes.push(normal.apply_axis_angle(normal.cross(toward).normalize(), angle));
        }
    }
    axes
}

/// Whether every span of the chain, the flat-enough ones aside, leans no
/// more than a trunk diagonal may at its length. A drop snapped to a node
/// can leave the span under the tip flatter than any routed diagonal.
fn holds_trunk_shape(points: &[Vec3], clearance_mm: f64) -> bool {
    points.windows(2).all(|pair| {
        let (a, b) = (pair[0], pair[1]);
        if distance_xy(a, b) <= js_max(clearance_mm * 0.5, 0.05) {
            return true;
        }
        let allowed = get_length_aware_max_angle_from_vertical_deg(
            distance_3d(a, b),
            TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG,
            Some(TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG),
        );
        span_lean_from_vertical_deg(a, b) <= allowed + 0.05
    })
}

/// A trunk's route for the contact of `input`: its socket, its chain to the
/// plate and its cone; or why it has none.
pub fn calculate_smart_placement_v3(
    input: &SmartPlacementV3Input<'_>,
    sdf: &mut SdfCache,
    matrix_world: Matrix4,
    stats: &mut RouterStats,
    settings: &SupportSettings,
    context: SmartPlacementV3Context,
) -> TrunkPlacementResult {
    let _perf = crate::perf::scope("router:v3");
    stats.placements += 1;
    let plain = calculate_standard_placement(&input.placement, settings);
    if plain.error.is_some() {
        return plain;
    }
    {
        let _p = crate::perf::scope("router:setup");
        sdf.refresh_matrix(matrix_world);
        sdf.enable_column_map(
            column_clearance_mm(settings.shaft.diameter_mm),
            COLUMN_MAP_CELL_MM,
        );
    }
    let again = |sdf: &mut SdfCache, stats: &mut RouterStats, context| {
        calculate_smart_placement_v3(input, sdf, matrix_world, stats, settings, context)
    };
    let Some(found) = route(input, sdf, stats, settings, context, plain) else {
        return again(
            sdf,
            stats,
            SmartPlacementV3Context {
                ignore_grid: true,
                ..context
            },
        );
    };
    if found.error != Some(LimitationCode::CollisionWithModel) || context.tight {
        return found;
    }
    let tight = again(
        sdf,
        stats,
        SmartPlacementV3Context {
            tight: true,
            ..context
        },
    );
    if tight.error.is_none() { tight } else { found }
}

/// One routing pass; `None` asks for the pass again with the grid off.
fn route(
    input: &SmartPlacementV3Input<'_>,
    sdf: &SdfCache,
    stats: &mut RouterStats,
    settings: &SupportSettings,
    context: SmartPlacementV3Context,
    plain: TrunkPlacementResult,
) -> Option<TrunkPlacementResult> {
    let p = &input.placement;
    let root_top_z = p.roots_top_z;
    let clearance = column_clearance_mm(settings.shaft.diameter_mm);
    let spacing = settings.grid.spacing_mm;
    let grid_dropped = settings.grid.enabled && context.ignore_grid;
    let grid = settings.grid.enabled && !context.ignore_grid;
    let pillars = grid && settings.grid.pillars;
    let rings = if grid { GRID_BASE_RINGS } else { BASE_RINGS };
    let mut checks = Checks {
        sdf,
        stats,
        tip: p.tip_pos,
        profile: p.tip_profile,
        clearance,
        disk_height: settings.roots.disk_height_mm,
        cone_height: settings.roots.cone_height_mm,
        root_radius: settings.roots.diameter_mm / 2.0,
        shaft_radius: settings.shaft.diameter_mm / 2.0,
        max_swing_deg: if context.tight {
            TIGHT_SWING_DEG
        } else {
            MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG
        },
        roots: HashMap::new(),
        cones: HashMap::new(),
    };
    let axis_hint = plain.cone_axis.unwrap_or(p.tip_normal);
    let socket = plain.socket_pos;
    let reach = js_min(72.0, js_max(48.0, js_max(0.0, socket.z - root_top_z) * 2.5));

    // Asked first for the cells they leave cached, which the rest reads.
    let _ = checks.segment_blocked(socket, Vec3::new(socket.x, socket.y, root_top_z));
    let _ = checks.root_blocked(socket.x, socket.y);

    // The cones to try: the policy's own (not in the tight pass), then the
    // swung ones, smallest swing first.
    let own = checks.cone_toward(socket, &[], p.tip_normal, axis_hint);
    let swings: &[f64] = if context.tight {
        &[TIGHT_SWING_DEG]
    } else {
        &CONE_SWINGS_DEG
    };
    let cones: Vec<(Cone, bool)> = (!context.tight)
        .then_some((own, false))
        .into_iter()
        .chain(
            swung_axes(p.tip_normal, axis_hint, swings)
                .into_iter()
                .map(|axis| {
                    (
                        Cone {
                            socket_pos: get_socket_position(
                                own.cone_start_pos,
                                axis,
                                Some(&p.tip_profile),
                            ),
                            cone_axis: axis,
                            cone_start_pos: own.cone_start_pos,
                        },
                        true,
                    )
                }),
        )
        .collect();

    // 1. A straight drop: the cone clears, its column clears and a root fits
    //    right under it.
    for &(cone, _) in &cones {
        checks.stats.cones_tested += 1;
        let s = cone.socket_pos;
        if checks.cone_blocked(cone.cone_start_pos, s)
            || !checks.contact_on_top(&cone)
            || checks.segment_blocked(s, Vec3::new(s.x, s.y, root_top_z))
            || checks.root_blocked(s.x, s.y)
        {
            continue;
        }
        let base = checks.base_for(Xy { x: s.x, y: s.y }, s, root_top_z, spacing, grid, rings);
        // A root the grid moved off the column is a diagonal's job.
        let Some(base) = base.filter(|b| {
            js_hypot(&[b.base_pos.x - s.x, b.base_pos.y - s.y]) <= STRAIGHT_TOLERANCE_MM
        }) else {
            continue;
        };
        if grid && !holds_trunk_shape(&[s, base.root_top], clearance) {
            return None;
        }
        return Some(TrunkPlacementResult {
            socket_pos: s,
            joints: Some(Vec::new()),
            construction_joints: Some(Vec::new()),
            base_pos: base.base_pos,
            unsnapped_bottom_pos: Some(s),
            snapped_node_key: base.node_key,
            cone_axis: Some(cone.cone_axis),
            error: None,
            grid_ignored: Some(grid_dropped),
            ..plain
        });
    }

    // 2. One diagonal to a column that clears, then straight down, from
    //    each cone that clears in turn; the way off the surface first.
    let routed_deg = js_max(
        ROUTED_FLOOR_DEG,
        90.0 - settings.grid.min_routed_trunk_angle_deg,
    );
    let outward = Vec3::new(axis_hint.x, axis_hint.y, 0.0);
    let outward = (outward.length_sq() > 1e-6).then_some(Xy {
        x: outward.x,
        y: outward.y,
    });
    for &(start, swung) in &cones {
        let from = start.socket_pos;
        checks.stats.cones_tested += 1;
        if checks.cone_blocked(start.cone_start_pos, from) || !checks.contact_on_top(&start) {
            continue;
        }
        let field: &dyn CollisionField = checks.sdf;
        let found = {
            let mut root_fits = |x: f64, y: f64| !checks.root_blocked(x, y);
            let _p = crate::perf::scope("router:joint-search");
            if grid {
                // The drop must land on a node: the node first, the joint
                // over it.
                find_grid_joint(
                    field,
                    from,
                    root_top_z,
                    GridJointSearchOptions {
                        clearance_mm: clearance,
                        spacing_mm: spacing,
                        max_lateral_mm: if pillars {
                            js_min(reach, spacing * PILLAR_REACH_STEPS)
                        } else {
                            reach
                        },
                        lean_from_vertical_deg: TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG,
                        min_vertical_leg_mm: MIN_DROP_MM,
                        max_node_count: if pillars { PILLAR_NODES } else { GRID_NODES },
                        preferred_direction: outward,
                        base_fits_at: &mut root_fits,
                        leans_deg: if pillars { &PILLAR_LEANS_DEG } else { &[] },
                    },
                )
            } else {
                find_escape_joint(
                    field,
                    from,
                    root_top_z,
                    EscapeJointSearchOptions {
                        clearance_mm: clearance,
                        step_mm: WALK_STEP_MM,
                        max_lateral_mm: reach,
                        lean_from_vertical_deg: TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG,
                        min_vertical_leg_mm: MIN_DROP_MM,
                        directions: build_direction_fan(outward, DIRECTION_COUNT),
                        base_fits_at: &mut root_fits,
                    },
                )
            }
        };
        let stats = &mut *checks.stats;
        stats.joint_searches += 1;
        stats.joint_probes += u64::from(found.probes);
        *stats
            .joint_outcomes
            .entry(found.outcome.as_str().to_owned())
            .or_insert(0) += 1;
        let Some(joint) = found.joint else {
            // No node gives the drop room at an allowed lean: the contact is
            // routed again with the grid off.
            if grid {
                return None;
            }
            continue;
        };
        let bits = 31 - found.probes.max(1).leading_zeros();
        let bucket = (bits.saturating_sub(6) as usize).min(stats.found_probe_buckets.len() - 1);
        stats.max_found_probes = stats.max_found_probes.max(u64::from(found.probes));
        stats.found_probe_buckets[bucket] += 1;

        let at = joint.joint;
        let joints = vec![at];
        let cone = checks.cone_toward(from, &joints, p.tip_normal, axis_hint);
        if checks.cone_blocked(cone.cone_start_pos, cone.socket_pos)
            || checks.segment_blocked(cone.socket_pos, at)
        {
            continue;
        }
        // A swung cone moved the socket: its diagonal is held to the lean the
        // search used at most.
        let lean = if swung {
            TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG
        } else {
            joint.lean_from_vertical_deg
        };
        if !segment_satisfies_max_angle_from_vertical(cone.socket_pos, at, lean + 0.05) {
            continue;
        }
        let Some(base) = checks.base_for(
            Xy { x: at.x, y: at.y },
            at,
            root_top_z,
            spacing,
            grid,
            rings,
        ) else {
            continue;
        };
        // 3. The drop clears and leans no more than its length allows.
        let drop_ok = !checks.segment_blocked(at, base.root_top)
            && segment_satisfies_length_aware_max_angle_from_vertical(
                at,
                base.root_top,
                js_min(89.0, routed_deg + DROP_SLACK_DEG),
                Some(routed_deg),
            );
        if !drop_ok {
            continue;
        }
        if grid && !holds_trunk_shape(&[cone.socket_pos, at, base.root_top], clearance) {
            return None;
        }
        return Some(TrunkPlacementResult {
            socket_pos: cone.socket_pos,
            joints: Some(joints),
            construction_joints: Some(Vec::new()),
            base_pos: base.base_pos,
            unsnapped_bottom_pos: Some(Vec3::new(at.x, at.y, 0.0)),
            snapped_node_key: base.node_key,
            cone_axis: Some(cone.cone_axis),
            error: None,
            grid_ignored: Some(grid_dropped),
            ..plain
        });
    }
    // Nothing attaches it here; the caller has the cavity bridge.
    Some(TrunkPlacementResult {
        error: Some(LimitationCode::CollisionWithModel),
        ..plain
    })
}

#[cfg(test)]
mod clearance_tests {
    use super::*;

    /// #233: shafts whose diameters differ by less than 0.05 mm ask for one
    /// clearance, never less room than their own radius gives.
    #[test]
    fn close_shafts_share_one_clearance_and_none_gets_less_room() {
        let mut clearances = std::collections::BTreeSet::new();
        for k in 0..=400 {
            let d = 1.0 + f64::from(k) * 0.001;
            let c = column_clearance_mm(d);
            let own = d / 2.0 + STANDOFF_MM;
            assert!(c >= own - 1e-12, "{d}: {c} < {own}");
            assert!(c - own <= CLEARANCE_RADIUS_STEP_MM + 1e-12, "{d}: {c}");
            clearances.insert((c * 1e6).round() as i64);
        }
        assert!(clearances.len() <= 9, "{}", clearances.len());
        assert_eq!(column_clearance_mm(1.1), 0.55 + STANDOFF_MM);
    }
}

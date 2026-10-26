//! DragonFruit `src/supports/PlacementLogicV3/SmartPlacementV3.ts`.
//!
//! SmartPlacementV3 — the support trunk router.
//!
//! One entry point, one route shape, one collision oracle. The shape it
//! produces, which is what slicers actually print:
//!
//! ```text
//! contact cone ──► one diagonal ──► joint ──► straight drop to the plate
//! ```
//!
//! The diagonal leaves the socket at up to 45° from vertical and is lifted as
//! high as its column allows, so the tilt is as short as the geometry permits
//! and everything below the joint is vertical. A trunk is strongest as a
//! vertical pillar, so the router's whole job is to find the earliest point
//! where vertical becomes possible and get there in one move.
//!
//! Cost is bounded by construction: the search is a bounded outward walk with a
//! hard probe budget (see `escape_joint_search`), the roots volume is the only
//! other place that touches the mesh, and nothing here iterates to a fixed
//! point.
//!
//! Differences from DragonFruit, none of which changes a placement:
//! - The mesh is its distance field: the caller passes the pooled [`SdfCache`]
//!   and the mesh's world matrix (`sdf.refreshMatrix()` reads `mesh.matrixWorld`).
//! - Settings are passed in (`getSettings()`), and the run counters are the
//!   caller's [`RouterStats`] rather than a module global.
//! - The pinned-preset column map prewarm runs on `requestIdleCallback`, which
//!   is a no-op off the browser main thread (DragonFruit's worker and Node);
//!   the GUI can call `SdfCache::prewarm_column_map` itself. The pathfinding
//!   debug overlay snapshot and the perf marks are not produced.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::escape_joint_search::{
    EscapeJointSearchOptions, GridJointSearchOptions, Xy, build_direction_fan, find_escape_joint,
    find_grid_joint,
};
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
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_socket_position;
use crate::supports::support_primitives::contact_cone::types::SupportTipProfile;
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use crate::supports::types::{LimitationCode, Vec3};
use crate::three::Matrix4;
use crate::three::math_utils::{deg_to_rad, js_cos, js_hypot, js_max, js_min, js_round, js_sin};

/// What one placement asks the geometry, counted. The router's cost is the
/// *number* of questions it puts to the distance field, so the run report needs
/// the counts to say which stage to attack. Reset per run by the auto-support
/// planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterStats {
    pub placements: u64,
    /// Cones the straight-drop loop tested (the straight one plus deviations).
    pub cones_tested: u64,
    /// Cone-vs-model checks that reached the distance field (memo misses).
    pub cone_gates: u64,
    /// Sockets the routed fallback handed to the joint search.
    pub joint_searches: u64,
    /// SDF probes those searches spent, summed.
    pub joint_probes: u64,
    /// Root-volume checks that were not already memoized for their XY.
    pub roots_checks: u64,
    /// SDF queries inside those checks (a slice centre plus its perimeter).
    pub roots_samples: u64,
    /// Base positions the ring search tried.
    pub base_candidates: u64,
    /// How the joint searches ended (`found`, `never-cleared`, `probe-budget`,
    /// `no-direction`), in first-seen order.
    pub joint_outcomes: IndexMap<String, u64>,
    /// Probes the searches that *found* a joint needed, bucketed by powers of
    /// two (index 0 = up to 64 probes, 1 = up to 128, ...).
    pub found_probe_buckets: [u64; 5],
    /// The largest probe count any successful search spent.
    pub max_found_probes: u64,
}

impl Default for RouterStats {
    fn default() -> Self {
        RouterStats {
            placements: 0,
            cones_tested: 0,
            cone_gates: 0,
            joint_searches: 0,
            joint_probes: 0,
            roots_checks: 0,
            roots_samples: 0,
            base_candidates: 0,
            joint_outcomes: IndexMap::new(),
            found_probe_buckets: [0; 5],
            max_found_probes: 0,
        }
    }
}

impl RouterStats {
    /// `resetRouterStats()`.
    pub fn reset(&mut self) {
        *self = RouterStats::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmartPlacementV3Input<'a> {
    pub placement: TrunkPlacementInput,
    pub model_id: &'a str,
    /// Hover preview rather than a committed click. Reported to the debug overlay only.
    pub is_preview: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SmartPlacementV3Context {
    /// Resolve as if the grid were off. Set only by the retry below: a grid
    /// placement that cannot meet the shape rule is re-resolved without it.
    pub ignore_grid: bool,
}

// Standoff from model geometry, matching the shaft collision gate used
// everywhere else in the support system.
const COLLISION_AVOIDANCE_MM: f64 = 0.48;

/// The clearance the router asks the column map for, from the shaft it clears.
/// One function for both the ask and the prewarm, so a prewarmed map cannot
/// miss the clearance the hover then asks for and rebuild anyway.
pub fn column_clearance_mm(shaft_diameter_mm: f64) -> f64 {
    shaft_diameter_mm / 2.0 + COLLISION_AVOIDANCE_MM
}

/// The column map's cell size, shared with the prewarm for the same reason.
pub const COLUMN_MAP_CELL_MM: f64 = 0.2;

/// How many preset slots the hotkeys can switch between.
pub const PINNED_PRESET_SLOT_COUNT: u32 = 6;

/// Safety margin applied to the roots volume, as the rest of the system does.
const ROOTS_DISK_SAFETY_MM: f64 = COLLISION_AVOIDANCE_MM;
/// Perimeter samples around the roots cross-section at each height slice.
const ROOTS_DISK_PERIMETER_SAMPLES: u32 = 16;
/// Ring search around the preferred base node when the grid is off. Grid mode
/// searches no rings at all: the joint already sits on a node, and when the node
/// under the joint cannot take the base, the answer is a different joint, not a
/// longer lean.
const MAX_BASE_SEARCH_RINGS: u32 = 4;
const GRID_BASE_SEARCH_RINGS: u32 = 0;
// Shortest vertical leg worth putting below a joint.
const MIN_VERTICAL_LEG_MM: f64 = 1.0;
/// A base this far off the socket's column still counts as a straight drop.
const STRAIGHT_BASE_TOLERANCE_MM: f64 = 0.05;
/// Lateral step of the outward walk. One SDF probe pair per step.
const WALK_STEP_MM: f64 = 0.5;
/// Directions tried, in preference order, before the router gives up.
const DIRECTION_COUNT: usize = 12;
/// How many lattice nodes the grid search may try, nearest first.
const GRID_JOINT_NODE_BUDGET: u32 = 24;
/// The vertical leg below the joint is the load-bearing span, so it obeys the
/// configured routed-trunk angle (90 minus `grid.minRoutedTrunkAngleDeg`).
const ROUTED_DETOUR_SLACK_DEG: f64 = 10.0;
const MIN_ALLOWED_ROUTED_ANGLE_DEG: f64 = 15.0;
/// How far the cone may swing off the surface normal when the cone at the
/// nominal socket is blocked, and how many directions around the tip are tried
/// at each angle. Smallest deviation first.
const CONE_DEVIATION_ANGLES_DEG: [f64; 3] = [10.0, 20.0, 30.0];
const CONE_DEVIATION_DIRECTION_COUNT: usize = 8;

/// A memo key part as JavaScript's string key makes it: `Math.round` of the
/// value, with -0 and 0 one key and every NaN one key.
fn memo_key_part(rounded: f64) -> u64 {
    if rounded == 0.0 {
        0.0f64.to_bits()
    } else if rounded.is_nan() {
        f64::NAN.to_bits()
    } else {
        rounded.to_bits()
    }
}

/// Roots volume check: does the disk + cone the root is made of intersect the model?
///
/// Samples the cross-section the root actually occupies at each height and asks
/// whether that circle comes within the safety margin of the mesh, with the
/// bounding-ball early-out the 1-Lipschitz SDF allows.
#[allow(clippy::too_many_arguments)]
fn roots_volume_blocked(
    sdf: &SdfCache,
    stats: &mut RouterStats,
    center_x: f64,
    center_y: f64,
    disk_height: f64,
    cone_height: f64,
    roots_radius: f64,
    shaft_radius: f64,
) -> bool {
    stats.roots_checks += 1;
    let root_top_z = disk_height + cone_height;
    let z_slices = js_max(4.0, (root_top_z / sdf.cell_size()).ceil());
    let mut slice = 0.0;
    while slice <= z_slices {
        let z = (slice / z_slices) * root_top_z;
        let radius_at_z = if z <= disk_height {
            roots_radius
        } else {
            roots_radius
                + ((z - disk_height) / js_max(cone_height, 1e-6)) * (shaft_radius - roots_radius)
        };

        stats.roots_samples += 1;
        let center_dist = sdf.distance_at(center_x, center_y, z);
        if center_dist >= radius_at_z + ROOTS_DISK_SAFETY_MM {
            slice += 1.0;
            continue;
        }
        if center_dist < ROOTS_DISK_SAFETY_MM {
            return true;
        }

        for i in 0..ROOTS_DISK_PERIMETER_SAMPLES {
            let angle = (f64::from(i) / f64::from(ROOTS_DISK_PERIMETER_SAMPLES))
                * std::f64::consts::PI
                * 2.0;
            stats.roots_samples += 1;
            if sdf.distance_at(
                center_x + js_cos(angle) * radius_at_z,
                center_y + js_sin(angle) * radius_at_z,
                z,
            ) < ROOTS_DISK_SAFETY_MM
            {
                return true;
            }
        }
        slice += 1.0;
    }
    false
}

/// A cone the router may commit: where it starts on the disk, its axis, and the
/// socket the shaft leaves from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cone {
    socket_pos: Vec3,
    cone_axis: Vec3,
    cone_start_pos: Vec3,
}

/// A committed base: where the root lands and where the vertical leg ends.
#[derive(Debug, Clone, PartialEq)]
struct Base {
    base_pos: Vec3,
    root_top_target: Vec3,
    node_key: Option<String>,
}

/// The per-placement gates and their memos, over the model's distance field.
struct Gates<'a> {
    sdf: &'a SdfCache,
    stats: &'a mut RouterStats,
    tip_pos: Vec3,
    tip_profile: SupportTipProfile,
    clearance_mm: f64,
    disk_height: f64,
    cone_height: f64,
    roots_radius: f64,
    shaft_radius: f64,
    // Two memos only: the roots volume (a disk + cone sweep, the most expensive
    // per-point check here) and the contact-cone gate. The segment test is
    // deliberately not memoized: the key costs more than the cell-cached call.
    roots_memo: HashMap<(u64, u64), bool>,
    cone_gate_memo: HashMap<[u64; 6], bool>,
}

impl Gates<'_> {
    /// True when the roots volume at this XY is blocked by the model.
    fn roots_blocked_at(&mut self, x: f64, y: f64) -> bool {
        let key = (
            memo_key_part(js_round(x / 0.05)),
            memo_key_part(js_round(y / 0.05)),
        );
        if let Some(&cached) = self.roots_memo.get(&key) {
            return cached;
        }
        let blocked = roots_volume_blocked(
            self.sdf,
            self.stats,
            x,
            y,
            self.disk_height,
            self.cone_height,
            self.roots_radius,
            self.shaft_radius,
        );
        self.roots_memo.insert(key, blocked);
        blocked
    }

    fn segment_blocked_between(&self, from: Vec3, to: Vec3) -> bool {
        self.sdf
            .segment_blocked_xyz(from.x, from.y, from.z, to.x, to.y, to.z, self.clearance_mm)
    }

    fn cone_blocked_at(&self, socket_pos: Vec3) -> bool {
        self.sdf
            .distance_at(socket_pos.x, socket_pos.y, socket_pos.z)
            < self.clearance_mm
    }

    /// The cone body, not just the socket point: a cone can lean sideways into
    /// geometry the socket itself clears. Memoized per placement, because the
    /// router tests the same cone geometry looking for a straight drop and again
    /// as a socket candidate for the joint search.
    fn contact_cone_blocked_at(&mut self, cone_start_pos: Vec3, socket_pos: Vec3) -> bool {
        let key = [
            cone_start_pos.x,
            cone_start_pos.y,
            cone_start_pos.z,
            socket_pos.x,
            socket_pos.y,
            socket_pos.z,
        ]
        .map(|value| memo_key_part(js_round(value * 100.0)));
        if let Some(&cached) = self.cone_gate_memo.get(&key) {
            return cached;
        }
        self.stats.cone_gates += 1;
        let blocked = is_contact_cone_blocked(
            self.sdf,
            ContactConeSweep {
                start: cone_start_pos,
                end: socket_pos,
                start_radius: self.tip_profile.contact_diameter_mm / 2.0,
                end_radius: self.tip_profile.body_diameter_mm / 2.0,
            },
        );
        self.cone_gate_memo.insert(key, blocked);
        blocked
    }

    /// The contact is the top of the member: no cone the router commits may lift
    /// its own socket end over the tip.
    fn cone_keeps_contact_on_top(&self, cone: &Cone) -> bool {
        cone_keeps_contact_highest(
            cone.cone_axis,
            cone.socket_pos,
            self.tip_pos,
            self.tip_profile.body_diameter_mm,
        )
    }

    /// Resolves the cone's axis and the shaft's top point together, because they
    /// are one decision: the builder renders the cone along the direction from
    /// the cone start to the socket.
    ///
    /// With a joint to aim at, the socket moves onto the line from the cone start
    /// toward that joint, so the cone and the first shaft segment form one line.
    /// The move is clamped to the cone's deviation budget off the surface normal,
    /// and dropped outright when it would lift the cone's socket end over the
    /// tip. With no joint (a straight shaft) the socket stands and the axis
    /// follows it.
    fn resolve_cone_socket_and_axis(
        &mut self,
        socket_pos: Vec3,
        joints: &[Vec3],
        tip_normal: Vec3,
        cone_axis_hint: Vec3,
    ) -> Cone {
        let tip_pos = self.tip_pos;
        let tip_profile = self.tip_profile;
        let disk_thickness = if tip_profile.is_disk() {
            calculate_disk_thickness(
                tip_normal,
                cone_axis_hint,
                Some(&tip_profile.as_disk_profile()),
            )
        } else {
            0.0
        };
        let cone_start_pos = Vec3::new(
            tip_pos.x + tip_normal.x * disk_thickness,
            tip_pos.y + tip_normal.y * disk_thickness,
            tip_pos.z + tip_normal.z * disk_thickness,
        );

        if let Some(&first_joint) = joints.first() {
            let aligned_axis = clamp_cone_axis_deviation_from_surface_normal(
                tip_normal,
                Vec3::new(
                    first_joint.x - cone_start_pos.x,
                    first_joint.y - cone_start_pos.y,
                    first_joint.z - cone_start_pos.z,
                )
                .normalize(),
                MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
            );
            let aligned_socket =
                get_socket_position(cone_start_pos, aligned_axis, Some(&tip_profile));
            if !self.cone_blocked_at(aligned_socket)
                && !self.contact_cone_blocked_at(cone_start_pos, aligned_socket)
                && !self.segment_blocked_between(aligned_socket, first_joint)
                && cone_keeps_contact_highest(
                    aligned_axis,
                    aligned_socket,
                    tip_pos,
                    tip_profile.body_diameter_mm,
                )
            {
                return Cone {
                    socket_pos: aligned_socket,
                    cone_axis: aligned_axis,
                    cone_start_pos,
                };
            }
        }

        Cone {
            socket_pos,
            cone_axis: Vec3::new(
                socket_pos.x - cone_start_pos.x,
                socket_pos.y - cone_start_pos.y,
                socket_pos.z - cone_start_pos.z,
            )
            .normalize(),
            cone_start_pos,
        }
    }

    /// Resolves where the root lands and where the shaft's vertical leg ends.
    ///
    /// With the grid on, the base snaps to the nearest legal node: legal means the
    /// roots volume fits there and the leg from the joint reaches it without
    /// clipping. Among legal nodes the nearest to directly under the joint wins.
    /// With the grid off the preferred XY is continuous and is used as-is.
    fn resolve_base(
        &mut self,
        preferred_xy: Xy,
        last_segment_start: Vec3,
        root_top_z: f64,
        spacing_mm: f64,
        grid_enabled: bool,
        max_search_rings: u32,
    ) -> Option<Base> {
        let node_keys = if grid_enabled {
            build_nearest_candidate_node_keys(
                &grid_node_key_from_xy(preferred_xy.x, preferred_xy.y, spacing_mm),
                max_search_rings,
            )
        } else {
            vec!["continuous".to_owned()]
        };

        let mut best: Option<(Base, f64)> = None;
        for node_key in node_keys {
            self.stats.base_candidates += 1;
            let (x, y) = if grid_enabled {
                grid_snapped_xy_from_key(&node_key, spacing_mm)
            } else {
                (preferred_xy.x, preferred_xy.y)
            };
            if self.roots_blocked_at(x, y) {
                continue;
            }

            let base_pos = Vec3::new(x, y, 0.0);
            let root_top_target = Vec3::new(x, y, root_top_z);
            if self.segment_blocked_between(last_segment_start, root_top_target) {
                continue;
            }

            let lateral_mm = js_hypot(&[x - last_segment_start.x, y - last_segment_start.y]);
            if best.as_ref().is_none_or(|(_, b)| lateral_mm < *b) {
                best = Some((
                    Base {
                        base_pos,
                        root_top_target,
                        node_key: grid_enabled.then_some(node_key),
                    },
                    lateral_mm,
                ));
            }
        }
        best.map(|(base, _)| base)
    }
}

/// Axes the cone can take to reach around geometry beside the tip.
///
/// The contact disk stays flat on the surface, so the cone may only swing away
/// from the surface normal by `MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG`.
/// Ordered smallest deviation first, then outward direction first, so the first
/// candidate that clears is the least-distorted cone.
fn build_cone_deviation_axes(tip_normal: Vec3, outward_hint: Vec3) -> Vec<Vec3> {
    let normal = tip_normal;
    if normal.length_sq() < 1e-10 {
        return Vec::new();
    }
    let normal = normal.normalize();

    // The fan is generated in 2D and mapped onto the plane the cone can tilt in.
    let reference = if normal.z.abs() < 0.9 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let tangent_x = Vec3::cross_vectors(reference, normal).normalize();
    let tangent_y = Vec3::cross_vectors(normal, tangent_x).normalize();

    let hint = outward_hint.add_scaled_vector(normal, -outward_hint.dot(normal));
    let preferred = if hint.length_sq() > 1e-6 {
        Xy {
            x: hint.dot(tangent_x),
            y: hint.dot(tangent_y),
        }
    } else {
        Xy { x: 1.0, y: 0.0 }
    };

    let mut axes = Vec::new();
    for angle_deg in CONE_DEVIATION_ANGLES_DEG {
        let angle_rad = deg_to_rad(js_min(
            angle_deg,
            MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
        ));
        for direction in build_direction_fan(Some(preferred), CONE_DEVIATION_DIRECTION_COUNT) {
            let tilt = tangent_x
                .multiply_scalar(direction.x)
                .add_scaled_vector(tangent_y, direction.y);
            // Rotating the normal about (normal × tilt) swings it toward tilt.
            axes.push(normal.apply_axis_angle(normal.cross(tilt).normalize(), angle_rad));
        }
    }
    axes
}

/// Whether a resolved chain holds the shape rule: every span from the socket down
/// to the base, measured against the 45 degrees a trunk's diagonal is built to.
///
/// Snapping the drop to a node leaves the socket where the contact is, so the
/// span under the tip can come out flatter than any diagonal the router would
/// ever have built.
fn chain_holds_shape_rule(points: &[Vec3], clearance_mm: f64) -> bool {
    for pair in points.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if distance_xy(start, end) <= js_max(clearance_mm * 0.5, 0.05) {
            continue;
        }
        let span_mm = distance_3d(start, end);
        // Short spans may take the detour slack, longer ones tighten to the
        // trunk's 45 degrees.
        let allowance_deg = get_length_aware_max_angle_from_vertical_deg(
            span_mm,
            TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG,
            Some(TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG),
        );
        if span_lean_from_vertical_deg(start, end) > allowance_deg + 0.05 {
            return false;
        }
    }
    true
}

/// Resolves a trunk placement: the socket the shaft leaves from, the chain that
/// gets it to the plate, and the cone that attaches it to the model.
///
/// Returns a `COLLISION_WITH_MODEL` result rather than searching harder when the
/// socket's column is blocked and no joint in the envelope fixes it. A trunk
/// that cannot reach the plate in one diagonal is not a trunk, and the caller
/// has the cavity fallback for exactly that case.
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
    let standard = calculate_standard_placement(&input.placement, settings);
    if standard.error.is_some() {
        return standard;
    }

    let setup = crate::perf::scope("router:setup");
    sdf.refresh_matrix(matrix_world);
    let clearance_mm = column_clearance_mm(settings.shaft.diameter_mm);
    // The router's columns are the bulk of this run's distance-field reads, and
    // the exact column map answers them from one scalar per XY cell. It is
    // clearance-specific: one map per clearance, reused after that.
    sdf.enable_column_map(clearance_mm, COLUMN_MAP_CELL_MM);
    drop(setup);

    match route(input, sdf, stats, settings, context, standard) {
        Some(result) => result,
        // The grid could not serve this contact at the shape rule: re-resolve
        // it as if the grid were off.
        None => calculate_smart_placement_v3(
            input,
            sdf,
            matrix_world,
            stats,
            settings,
            SmartPlacementV3Context { ignore_grid: true },
        ),
    }
}

/// The routing body. `None` asks the caller to retry with the grid ignored.
fn route(
    input: &SmartPlacementV3Input<'_>,
    sdf: &SdfCache,
    stats: &mut RouterStats,
    settings: &SupportSettings,
    context: SmartPlacementV3Context,
    standard: TrunkPlacementResult,
) -> Option<TrunkPlacementResult> {
    let placement = &input.placement;
    let root_top_z = placement.roots_top_z;
    let clearance_mm = column_clearance_mm(settings.shaft.diameter_mm);
    let spacing_mm = settings.grid.spacing_mm;
    let ignored_grid = settings.grid.enabled && context.ignore_grid;
    let grid_enabled = settings.grid.enabled && !context.ignore_grid;
    let base_rings = if grid_enabled {
        GRID_BASE_SEARCH_RINGS
    } else {
        MAX_BASE_SEARCH_RINGS
    };

    let mut gates = Gates {
        sdf,
        stats,
        tip_pos: placement.tip_pos,
        tip_profile: placement.tip_profile,
        clearance_mm,
        disk_height: settings.roots.disk_height_mm,
        cone_height: settings.roots.cone_height_mm,
        roots_radius: settings.roots.diameter_mm / 2.0,
        shaft_radius: settings.shaft.diameter_mm / 2.0,
        roots_memo: HashMap::new(),
        cone_gate_memo: HashMap::new(),
    };

    let cone_axis_hint = standard.cone_axis.unwrap_or(placement.tip_normal);
    // Built only when needed: the walk around the tip is the rare case.
    let mut deviation_axes: Option<Vec<Vec3>> = None;
    let mut deviation_cones = |cone_start_pos: Vec3| -> Vec<Cone> {
        deviation_axes
            .get_or_insert_with(|| build_cone_deviation_axes(placement.tip_normal, cone_axis_hint))
            .iter()
            .map(|&axis| Cone {
                socket_pos: get_socket_position(cone_start_pos, axis, Some(&placement.tip_profile)),
                cone_axis: axis,
                cone_start_pos,
            })
            .collect()
    };

    let socket_pos = standard.socket_pos;
    let vertical_span_mm = js_max(0.0, socket_pos.z - root_top_z);
    let max_lateral_mm = js_min(72.0, js_max(48.0, vertical_span_mm * 2.5));

    // DragonFruit reports these two to the debug overlay. They are asked here
    // all the same, because the answers they cache shape what comes after.
    let _straight_preflight_clear = !gates.segment_blocked_between(
        socket_pos,
        Vec3::new(socket_pos.x, socket_pos.y, root_top_z),
    );
    let _roots_fit_straight_down = !gates.roots_blocked_at(socket_pos.x, socket_pos.y);

    // 1. Straight: the column below the socket is clear and a root fits there.
    //    The cone comes first and may walk around the tip; routing below gets
    //    its turn only when no deviation clears.
    let straight_cone =
        gates.resolve_cone_socket_and_axis(socket_pos, &[], placement.tip_normal, cone_axis_hint);
    let deviated = deviation_cones(straight_cone.cone_start_pos);
    for candidate in std::iter::once(straight_cone).chain(deviated.iter().copied()) {
        gates.stats.cones_tested += 1;
        if gates.contact_cone_blocked_at(candidate.cone_start_pos, candidate.socket_pos) {
            continue;
        }
        if !gates.cone_keeps_contact_on_top(&candidate) {
            continue;
        }
        let column_end = Vec3::new(candidate.socket_pos.x, candidate.socket_pos.y, root_top_z);
        if gates.segment_blocked_between(candidate.socket_pos, column_end) {
            continue;
        }
        if gates.roots_blocked_at(candidate.socket_pos.x, candidate.socket_pos.y) {
            continue;
        }
        let base = gates.resolve_base(
            Xy {
                x: candidate.socket_pos.x,
                y: candidate.socket_pos.y,
            },
            candidate.socket_pos,
            root_top_z,
            spacing_mm,
            grid_enabled,
            base_rings,
        );
        // A base the grid moved off the socket's column is not a straight drop:
        // it is left to the routing below, which is where a diagonal belongs.
        let Some(base) = base.filter(|base| {
            js_hypot(&[
                base.base_pos.x - candidate.socket_pos.x,
                base.base_pos.y - candidate.socket_pos.y,
            ]) <= STRAIGHT_BASE_TOLERANCE_MM
        }) else {
            continue;
        };

        let straight_chain = [candidate.socket_pos, base.root_top_target];
        if grid_enabled && !chain_holds_shape_rule(&straight_chain, clearance_mm) {
            return None;
        }
        return Some(TrunkPlacementResult {
            socket_pos: candidate.socket_pos,
            joints: Some(Vec::new()),
            construction_joints: Some(Vec::new()),
            base_pos: base.base_pos,
            unsnapped_bottom_pos: Some(candidate.socket_pos),
            snapped_node_key: base.node_key,
            cone_axis: Some(candidate.cone_axis),
            error: None,
            grid_ignored: Some(ignored_grid),
            ..standard
        });
    }

    // 2. One diagonal to a column that clears, then straight down. The fan is
    //    ordered so the way off the surface the tip is attached to is tried
    //    first; every direction is tried before giving up.
    let configured_routed_angle_deg = js_max(
        MIN_ALLOWED_ROUTED_ANGLE_DEG,
        90.0 - settings.grid.min_routed_trunk_angle_deg,
    );
    let outward = Vec3::new(cone_axis_hint.x, cone_axis_hint.y, 0.0);
    let outward_xy = (outward.length_sq() > 1e-6).then_some(Xy {
        x: outward.x,
        y: outward.y,
    });

    // The cone decides which socket the chain starts from: the cheap cone test
    // first, and the expensive joint search once per socket that clears.
    let socket_candidates =
        std::iter::once((straight_cone, false)).chain(deviated.iter().map(|&cone| (cone, true)));
    for (socket_candidate, is_deviated) in socket_candidates {
        let start_socket = socket_candidate.socket_pos;
        gates.stats.cones_tested += 1;
        if gates.contact_cone_blocked_at(socket_candidate.cone_start_pos, start_socket) {
            continue;
        }
        if !gates.cone_keeps_contact_on_top(&socket_candidate) {
            continue;
        }

        // Grid mode searches the lattice: the drop has to land on a node, so the
        // node is chosen first and the joint derived from it. Every other mode
        // drops at the first column that clears.
        let field: &dyn CollisionField = gates.sdf;
        let found = {
            let mut base_fits_at = |x: f64, y: f64| !gates.roots_blocked_at(x, y);
            let _p = crate::perf::scope("router:joint-search");
            if grid_enabled {
                find_grid_joint(
                    field,
                    start_socket,
                    root_top_z,
                    GridJointSearchOptions {
                        clearance_mm,
                        spacing_mm,
                        max_lateral_mm,
                        lean_from_vertical_deg: TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG,
                        min_vertical_leg_mm: MIN_VERTICAL_LEG_MM,
                        max_node_count: GRID_JOINT_NODE_BUDGET,
                        // Keep leaving the way the cone points.
                        preferred_direction: outward_xy,
                        base_fits_at: &mut base_fits_at,
                    },
                )
            } else {
                find_escape_joint(
                    field,
                    start_socket,
                    root_top_z,
                    EscapeJointSearchOptions {
                        clearance_mm,
                        step_mm: WALK_STEP_MM,
                        max_lateral_mm,
                        lean_from_vertical_deg: TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG,
                        min_vertical_leg_mm: MIN_VERTICAL_LEG_MM,
                        directions: build_direction_fan(outward_xy, DIRECTION_COUNT),
                        base_fits_at: &mut base_fits_at,
                    },
                )
            }
        };
        let stats = &mut *gates.stats;
        stats.joint_searches += 1;
        stats.joint_probes += u64::from(found.probes);
        *stats
            .joint_outcomes
            .entry(found.outcome.as_str().to_owned())
            .or_insert(0) += 1;
        let Some(joint) = found.joint else {
            // The grid could not serve this contact: no node left room for the
            // drop at a lean the shape rule allows. The contact is reachable,
            // just not on a node, so the grid is dropped rather than refused.
            if grid_enabled {
                return None;
            }
            continue;
        };
        // floor(log2(max(1, probes))) is the probe count's bit length less one.
        let log2 = 31 - found.probes.max(1).leading_zeros();
        let bucket = (log2.saturating_sub(6) as usize).min(stats.found_probe_buckets.len() - 1);
        stats.max_found_probes = stats.max_found_probes.max(u64::from(found.probes));
        stats.found_probe_buckets[bucket] += 1;

        let joint_pos = joint.joint;
        let joint_lean_deg = joint.lean_from_vertical_deg;
        let joints = vec![joint_pos];
        // The cone the chain leaves from: aimed at the joint when that lines the
        // cone up with the shaft, otherwise the socket that cleared.
        let cone = gates.resolve_cone_socket_and_axis(
            start_socket,
            &joints,
            placement.tip_normal,
            cone_axis_hint,
        );
        if gates.contact_cone_blocked_at(cone.cone_start_pos, cone.socket_pos) {
            continue;
        }
        if gates.segment_blocked_between(cone.socket_pos, joint_pos) {
            continue;
        }
        // A deviated cone moved the socket, so its diagonal is judged against the
        // ceiling the joint search itself used.
        let ceiling_deg = if is_deviated {
            TRUNK_DIAGONAL_LEAN_FROM_VERTICAL_DEG
        } else {
            joint_lean_deg
        };
        if !segment_satisfies_max_angle_from_vertical(
            cone.socket_pos,
            joint_pos,
            ceiling_deg + 0.05,
        ) {
            continue;
        }

        let Some(base) = gates.resolve_base(
            Xy {
                x: joint_pos.x,
                y: joint_pos.y,
            },
            joint_pos,
            root_top_z,
            spacing_mm,
            grid_enabled,
            base_rings,
        ) else {
            continue;
        };

        // 3. The leg angles are checked against the real geometry. The diagonal
        //    is the one segment exempt from the length-aware tightening, bounded
        //    by the lean ceiling instead.
        let vertical_leg_ok = !gates.segment_blocked_between(joint_pos, base.root_top_target)
            && segment_satisfies_length_aware_max_angle_from_vertical(
                joint_pos,
                base.root_top_target,
                js_min(89.0, configured_routed_angle_deg + ROUTED_DETOUR_SLACK_DEG),
                Some(configured_routed_angle_deg),
            );
        if !vertical_leg_ok {
            continue;
        }

        // Same rule on the whole chain: the leg below the joint can be the span
        // the grid made flat.
        if grid_enabled
            && !chain_holds_shape_rule(
                &[cone.socket_pos, joint_pos, base.root_top_target],
                clearance_mm,
            )
        {
            return None;
        }

        return Some(TrunkPlacementResult {
            socket_pos: cone.socket_pos,
            joints: Some(joints),
            construction_joints: Some(Vec::new()),
            base_pos: base.base_pos,
            unsnapped_bottom_pos: Some(Vec3::new(joint_pos.x, joint_pos.y, 0.0)),
            snapped_node_key: base.node_key,
            cone_axis: Some(cone.cone_axis),
            error: None,
            grid_ignored: Some(ignored_grid),
            ..standard
        });
    }

    // Nothing here can attach this contact: the caller has the cavity fallback
    // for exactly this case.
    Some(TrunkPlacementResult {
        error: Some(LimitationCode::CollisionWithModel),
        ..standard
    })
}

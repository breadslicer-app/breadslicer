//! Kickstands added by auto-bracing to steady tall trunks.
//!
//! Trunks joined by braces form a group, and a group is steady once its
//! braces run along two directions far enough apart. A tall trunk in a
//! group short of that gets kickstands: each dropped from a point high on the
//! trunk to a root beside it, parallel to the trunk, on a neighbouring grid
//! node when the grid is on, clear of other roots, shafts and the model. Per
//! direction the root nearest the trunk wins. A second, wider pass then
//! retries the groups still short of two directions.

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};

use super::kickstand_builder::{KickstandBuildInput, build_kickstand_data};
use super::types::{KickstandBuildResult, KickstandHostTarget};
use crate::geom::math_utils::{js_atan2, js_cos, js_max, js_min, js_sin};
use crate::supports::support_kinds::store_type_of;
use crate::supports::auto_bracing::mesh_clearance::{
    BraceMesh, BraceMeshes, line_passes_mesh_clearance,
};
use crate::supports::auto_bracing::settings::{AutoBracingSettings, auto_bracing_hard_rules};
use crate::supports::auto_bracing::two_axis_detection::{
    additional_axes_needed_for_two_axis_bracing, has_qualified_two_axis_bracing,
    normalize_axis_angle_rad,
};
use crate::supports::auto_support::types::js_compare;
use crate::supports::placement_logic::grid::grid_math::snap_to_grid_index;
use crate::supports::settings::types::SupportSettings;
use crate::supports::shafts::{EndpointHosts, ShaftEntity, resolve_segment_endpoints};
use crate::supports::support_kinds::{SupportTypeId, is_kickstand_host_type};
use crate::supports::types::{Kickstand, Knot, Roots, SupportState, Trunk, Vec3};
use crate::volume_analysis::islands::voxel_footprint::number_key;

/// Trunks shorter than this need no kickstand, in mm.
const MIN_TRUNK_HEIGHT_MM: f64 = 15.0;
/// Points tested along a kickstand's slanted drop for model clearance.
const DROP_SAMPLES: usize = 20;
/// The least distance a new root keeps from any other root, and from the
/// trunk it props, in mm.
const ROOT_CLEARANCE_MM: f64 = 2.0;
const ROOT_CLEARANCE_SQ: f64 = ROOT_CLEARANCE_MM * ROOT_CLEARANCE_MM;
const SEPARATION_DEG: f64 = auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG;
const HALF_PI: f64 = std::f64::consts::PI / 2.0;

/// The kickstands, roots and knots of the kickstands a regeneration keeps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StabiliserState {
    pub kickstands: IndexMap<String, Kickstand>,
    pub roots: IndexMap<String, Roots>,
    pub knots: IndexMap<String, Knot>,
}

/// A brace between two trunks, and the direction it runs in.
#[derive(Debug, Clone, PartialEq)]
pub struct BraceAxisEdge {
    pub a: String,
    pub b: String,
    pub angle_rad: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSnap {
    pub enabled: bool,
    pub spacing_mm: f64,
}

/// A point on a trunk a kickstand may attach to.
struct Anchor {
    segment_id: String,
    t: f64,
    pos: Vec3,
    diameter_mm: f64,
}

type GridNode = (String, u64, u64);

fn grid_node(model_id: &str, gx: f64, gy: f64) -> GridNode {
    (model_id.to_owned(), number_key(gx), number_key(gy))
}

fn need_after(axes: &[f64]) -> u8 {
    additional_axes_needed_for_two_axis_bracing(axes, SEPARATION_DEG)
}

fn steady(axes: &[f64]) -> bool {
    has_qualified_two_axis_bracing(axes, SEPARATION_DEG)
}

fn flat_distance_sq(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx * dx + dy * dy
}

/// Where roots already stand: their positions, and their grid nodes when the
/// grid is on.
struct Taken {
    roots: Vec<Vec3>,
    nodes: HashSet<GridNode>,
    grid_spacing: Option<f64>,
}

impl Taken {
    fn near_a_root(&self, x: f64, y: f64) -> bool {
        self.roots
            .iter()
            .any(|r| flat_distance_sq((r.x, r.y), (x, y)) <= ROOT_CLEARANCE_SQ)
    }

    fn take(&mut self, model_id: &str, pos: Vec3) {
        if let Some(spacing) = self.grid_spacing {
            self.nodes.insert(grid_node(
                model_id,
                snap_to_grid_index(pos.x, spacing),
                snap_to_grid_index(pos.y, spacing),
            ));
        }
        self.roots.push(pos);
    }
}

/// Trunks joined by braces, and for each group its brace directions and how
/// many more it needs.
#[derive(Default)]
struct BraceGroups {
    group_of: HashMap<String, String>,
    axes: HashMap<String, Vec<f64>>,
    need: HashMap<String, u8>,
}

impl BraceGroups {
    /// Groups are the connected sets of trunks over trunk-to-trunk braces,
    /// each named after its first trunk.
    fn of(trunk_ids: Vec<&str>, edges: &[BraceAxisEdge]) -> Self {
        let mut neighbours: IndexMap<&str, IndexSet<&str>> =
            trunk_ids.iter().map(|id| (*id, IndexSet::new())).collect();
        for edge in edges {
            let (a, b) = (edge.a.as_str(), edge.b.as_str());
            if !neighbours.contains_key(a) || !neighbours.contains_key(b) {
                continue;
            }
            neighbours.get_mut(a).expect("listed").insert(b);
            neighbours.get_mut(b).expect("listed").insert(a);
        }

        let mut groups = BraceGroups::default();
        let mut seen: HashSet<&str> = HashSet::new();
        for start in trunk_ids {
            if !seen.insert(start) {
                continue;
            }
            // Breadth first from `start`.
            let mut members = vec![start];
            let mut next = 0;
            while next < members.len() {
                let current = members[next];
                next += 1;
                for &n in neighbours.get(current).into_iter().flatten() {
                    if seen.insert(n) {
                        members.push(n);
                    }
                }
            }
            let member_set: HashSet<&str> = members.iter().copied().collect();
            let axes: Vec<f64> = edges
                .iter()
                .filter(|e| member_set.contains(e.a.as_str()) && member_set.contains(e.b.as_str()))
                .map(|e| e.angle_rad)
                .collect();
            let group = start.to_owned();
            groups.need.insert(group.clone(), need_after(&axes));
            groups.axes.insert(group.clone(), axes);
            for member in members {
                groups.group_of.insert(member.to_owned(), group.clone());
            }
        }
        groups
    }
}

/// The direction of every brace or kept kickstand at each trunk.
fn axes_at_trunks(
    trunks: &IndexMap<String, Trunk>,
    kept: &StabiliserState,
    edges: &[BraceAxisEdge],
) -> HashMap<String, Vec<f64>> {
    let mut axes: HashMap<String, Vec<f64>> = HashMap::new();
    for edge in edges {
        axes.entry(edge.a.clone()).or_default().push(edge.angle_rad);
        axes.entry(edge.b.clone()).or_default().push(edge.angle_rad);
    }
    let mut trunk_of_segment: HashMap<&str, &str> = HashMap::new();
    for trunk in trunks.values() {
        for segment in &trunk.segments {
            trunk_of_segment.insert(segment.id.as_str(), trunk.base.id.as_str());
        }
    }
    for kickstand in kept.kickstands.values() {
        let Some(trunk_id) = trunk_of_segment.get(kickstand.host_segment_id.as_str()) else {
            continue;
        };
        let (Some(root), Some(knot)) = (
            kept.roots.get(&kickstand.root_id),
            kept.knots.get(&kickstand.host_knot_id),
        ) else {
            continue;
        };
        let angle = normalize_axis_angle_rad(js_atan2(
            root.transform.pos.y - knot.pos.y,
            root.transform.pos.x - knot.pos.x,
        ));
        axes.entry((*trunk_id).to_owned()).or_default().push(angle);
    }
    axes
}

/// Points along every segment of the trunk a kickstand may attach to, and
/// the height of the trunk's top.
fn anchors_on(trunk: &Trunk, root: &Roots) -> (Vec<Anchor>, f64) {
    let mut top_z = root.transform.pos.z;
    let mut anchors = Vec::new();
    for (index, segment) in trunk.segments.iter().enumerate() {
        let hosts = EndpointHosts {
            root: Some(root),
            host_knot: None,
        };
        let Some(ends) =
            resolve_segment_endpoints(ShaftEntity::Trunk(trunk), segment, index, hosts)
        else {
            continue;
        };
        let (start, end) = (ends.start, ends.end);
        if end.z > top_z {
            top_z = end.z;
        }
        let mut t = 0.9;
        while t >= 0.1 {
            anchors.push(Anchor {
                segment_id: segment.id.clone(),
                t,
                pos: Vec3::new(
                    start.x + (end.x - start.x) * t,
                    start.y + (end.y - start.y) * t,
                    start.z + (end.z - start.z) * t,
                ),
                diameter_mm: segment.diameter,
            });
            t -= 0.2;
        }
    }
    (anchors, top_z)
}

/// The model clearance tests a kickstand must pass. Every model on the plate
/// is tested, since a kickstand can land beside a neighbour (#445).
struct Clearance<'m, 'a> {
    meshes: &'m BraceMeshes<'a>,
}

impl Clearance<'_, '_> {
    /// Whether the straight run from `a` to `b`, `diameter_mm` wide, keeps
    /// clear of every model.
    fn line_clear(&self, a: Vec3, b: Vec3, diameter_mm: f64) -> bool {
        self.meshes
            .values()
            .all(|mesh| line_passes_mesh_clearance(a, b, Some(mesh), diameter_mm))
    }

    /// Whether any point along the drop from `top` to `bottom` comes within
    /// `radius` of a model.
    fn drop_touches(&self, top: Vec3, bottom: Vec3, radius: f64) -> bool {
        self.meshes.values().any(|entry: &BraceMesh<'_>| {
            let to_local = entry.matrix.invert();
            let start = top.apply_matrix4(&to_local);
            let end = bottom.apply_matrix4(&to_local);
            (0..=DROP_SAMPLES).any(|i| {
                let t = i as f64 / DROP_SAMPLES as f64;
                entry
                    .bvh
                    .closest_point_to_point(start.lerp(end, t), 0.0, f64::INFINITY)
                    .is_some_and(|hit| hit.distance < radius)
            })
        })
    }

    /// Whether the built kickstand, root top to host, keeps clear of every
    /// model.
    fn kickstand_clear(&self, build: &KickstandBuildResult) -> bool {
        let body = js_max(0.001, build.kickstand.profile.body_diameter_mm);
        let root = &build.root;
        let mut path = vec![Vec3::new(
            root.transform.pos.x,
            root.transform.pos.y,
            root.transform.pos.z + root.disk_height + root.cone_height,
        )];
        for segment in &build.kickstand.segments {
            path.extend(segment.bottom_joint.as_ref().map(|j| j.pos));
            path.extend(segment.top_joint.as_ref().map(|j| j.pos));
        }
        path.push(build.host_knot.pos);
        path.windows(2).all(|run| {
            let (a, b) = (run[0], run[1]);
            let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
            dx * dx + dy * dy + dz * dz < 0.000001 || self.line_clear(a, b, body)
        })
    }
}

/// The free grid node next to the trunk's own (left, right, front or back)
/// nearest `(x, y)`, snapped back onto the grid; `None` when none is free or
/// the snap leaves the trunk's neighbourhood.
fn neighbour_node(
    taken: &Taken,
    model_id: &str,
    trunk_root: Vec3,
    (x, y): (f64, f64),
    spacing: f64,
) -> Option<(f64, f64)> {
    let gx = snap_to_grid_index(trunk_root.x, spacing);
    let gy = snap_to_grid_index(trunk_root.y, spacing);
    let mut nearest: Option<(f64, f64, f64)> = None;
    for (nx, ny) in [
        (gx + 1.0, gy),
        (gx - 1.0, gy),
        (gx, gy + 1.0),
        (gx, gy - 1.0),
    ] {
        let (wx, wy) = (nx * spacing, ny * spacing);
        if taken.nodes.contains(&grid_node(model_id, nx, ny)) || taken.near_a_root(wx, wy) {
            continue;
        }
        let distance_sq = flat_distance_sq((wx, wy), (x, y));
        if nearest.is_none_or(|n| distance_sq < n.2) {
            nearest = Some((wx, wy, distance_sq));
        }
    }
    let (wx, wy, _) = nearest?;
    let x = snap_to_grid_index(wx, spacing) * spacing;
    let y = snap_to_grid_index(wy, spacing) * spacing;
    let off_x = (snap_to_grid_index(x, spacing) - gx).abs();
    let off_y = (snap_to_grid_index(y, spacing) - gy).abs();
    if off_x > 1.0 || off_y > 1.0 {
        return None;
    }
    Some((x, y))
}

/// Whether a root at `(x, y)` keeps clear of the anchor, of the trunk's own
/// root, and of the trunk's footprint between them.
fn clear_of_trunk(anchor: Vec3, trunk_root: Vec3, (x, y): (f64, f64)) -> bool {
    if flat_distance_sq((x, y), (anchor.x, anchor.y)) <= ROOT_CLEARANCE_SQ
        || flat_distance_sq((x, y), (trunk_root.x, trunk_root.y)) <= ROOT_CLEARANCE_SQ
    {
        return false;
    }
    let (sx, sy) = (anchor.x - trunk_root.x, anchor.y - trunk_root.y);
    let length_sq = sx * sx + sy * sy;
    if length_sq > 0.000001 {
        let t = js_max(
            0.0,
            js_min(
                1.0,
                ((x - trunk_root.x) * sx + (y - trunk_root.y) * sy) / length_sq,
            ),
        );
        let (cx, cy) = (x - (trunk_root.x + sx * t), y - (trunk_root.y + sy * t));
        if cx * cx + cy * cy <= ROOT_CLEARANCE_SQ {
            return false;
        }
    }
    true
}

/// The kickstands auto-bracing adds this pass.
///
/// `live` are the settings kickstands are built with, and `store` the
/// committed state that names an unstamped trunk's type. Trial builds take
/// ids whether or not they are kept.
#[allow(clippy::too_many_arguments)]
pub fn generate_required_kickstands(
    snapshot: &SupportState,
    kickstand_state: &StabiliserState,
    settings: &AutoBracingSettings,
    existing_brace_edges: &[BraceAxisEdge],
    grid: GridSnap,
    meshes: &BraceMeshes<'_>,
    live: &SupportSettings,
    store: &SupportState,
    new_id: &mut dyn FnMut() -> String,
) -> Vec<KickstandBuildResult> {
    let collections = &snapshot.collections;
    let max_run = settings.max_brace_length_mm;
    let drop_distance = js_min(5.0, max_run * 0.8);
    let grid_spacing = (grid.enabled && grid.spacing_mm > 0.0).then_some(grid.spacing_mm);
    let clearance = Clearance { meshes };

    let mut taken = Taken {
        roots: Vec::new(),
        nodes: HashSet::new(),
        grid_spacing,
    };
    let all_roots = || {
        collections
            .roots
            .values()
            .chain(kickstand_state.roots.values())
    };
    taken.roots = all_roots().map(|r| r.transform.pos).collect();
    if let Some(spacing) = grid_spacing {
        for root in all_roots() {
            taken.nodes.insert(grid_node(
                &root.base.model_id,
                snap_to_grid_index(root.transform.pos.x, spacing),
                snap_to_grid_index(root.transform.pos.y, spacing),
            ));
        }
    }

    let axes_at = axes_at_trunks(&collections.trunks, kickstand_state, existing_brace_edges);
    let mut groups = BraceGroups::of(
        collections.trunks.keys().map(String::as_str).collect(),
        existing_brace_edges,
    );

    let mut generated = Vec::new();
    for cleanup in [false, true] {
        for trunk in collections.trunks.values() {
            let Some(root) = collections.roots.get(&trunk.root_id) else {
                continue;
            };
            let Some(host_kind) = trunk
                .base
                .type_id
                .or_else(|| store_type_of(&store.collections, &trunk.base.id))
                .filter(|t| is_kickstand_host_type(t.as_str()))
            else {
                continue;
            };
            let (mut anchors, top_z) = anchors_on(trunk, root);
            if top_z - root.transform.pos.z < MIN_TRUNK_HEIGHT_MM {
                continue;
            }

            let trunk_id = trunk.base.id.as_str();
            let group = groups
                .group_of
                .get(trunk_id)
                .cloned()
                .unwrap_or_else(|| trunk_id.to_owned());
            let group_axes = groups.axes.get(&group).cloned().unwrap_or_default();
            let mut need = groups.need.get(&group).copied().unwrap_or(0);
            if steady(&group_axes) || need == 0 || anchors.is_empty() {
                continue;
            }
            let wanted = need.clamp(1, 2);
            // Highest first, so a kickstand attaches as high as it can.
            anchors.sort_by(|a, b| js_compare(b.pos.z - a.pos.z));

            let mut reference_axis = group_axes
                .first()
                .or_else(|| axes_at.get(trunk_id).and_then(|axes| axes.first()))
                .copied()
                .unwrap_or(0.0);
            let mut axes_so_far = group_axes;
            let mut roots_placed: Vec<Vec3> = Vec::new();

            for i in 0..wanted {
                if steady(&axes_so_far) || need == 0 {
                    break;
                }
                let aim = if wanted == 2 && i == 0 {
                    0.0
                } else {
                    reference_axis + HALF_PI
                };
                let trial = TrialContext {
                    trunk,
                    root_pos: root.transform.pos,
                    host_kind,
                    cleanup,
                    aim,
                    reference_axis,
                    drop_distance,
                    max_run,
                    grid_spacing,
                    taken: &taken,
                    roots_placed: &roots_placed,
                    clearance: &clearance,
                    live,
                };
                let Some((build, root_pos)) = trial.best_drop(&anchors, new_id) else {
                    continue;
                };

                let trunk_root = root.transform.pos;
                let angle = normalize_axis_angle_rad(js_atan2(
                    root_pos.y - trunk_root.y,
                    root_pos.x - trunk_root.x,
                ));
                let axes = groups.axes.entry(group.clone()).or_default();
                let before = need_after(axes);
                let mut with_new = axes.clone();
                with_new.push(angle);
                let after = need_after(&with_new);
                // Kept only when it brings the group closer to two directions.
                if after >= before {
                    continue;
                }
                generated.push(build);
                taken.take(&trunk.base.model_id, root_pos);
                axes_so_far.push(angle);
                axes.push(angle);
                need = after;
                groups.need.insert(group.clone(), need);
                if i == 0 {
                    reference_axis = angle;
                }
                roots_placed.push(root_pos);
            }
        }
    }
    generated
}

/// What finding one kickstand for a trunk works with.
struct TrialContext<'s, 'm, 'a> {
    trunk: &'s Trunk,
    root_pos: Vec3,
    host_kind: SupportTypeId,
    cleanup: bool,
    /// The direction tried first.
    aim: f64,
    /// The group's first direction, which the cleanup pass also tries
    /// square to and along.
    reference_axis: f64,
    drop_distance: f64,
    max_run: f64,
    grid_spacing: Option<f64>,
    taken: &'s Taken,
    roots_placed: &'s [Vec3],
    clearance: &'s Clearance<'m, 'a>,
    live: &'s SupportSettings,
}

impl TrialContext<'_, '_, '_> {
    /// The direction and drop distance of attempt `attempt`. Attempts
    /// alternate between the aim and its opposite; the cleanup pass also
    /// swings square to and along the reference direction.
    fn attempt(&self, attempt: usize) -> (f64, f64) {
        let mut angle = self.aim
            + if attempt.is_multiple_of(2) {
                0.0
            } else {
                std::f64::consts::PI
            };
        if self.cleanup {
            let r = self.reference_axis;
            match attempt % 6 {
                2 => angle = r + HALF_PI,
                3 => angle = r + (3.0 * std::f64::consts::PI) / 2.0,
                4 => angle = r,
                5 => angle = r + std::f64::consts::PI,
                _ => {}
            }
        }
        let distance = if self.grid_spacing.is_some() {
            self.drop_distance
        } else {
            self.drop_distance + (attempt / 2) as f64 * 2.0
        };
        (angle, distance)
    }

    /// The legal kickstand whose root is nearest the trunk, from the highest
    /// anchor that has one.
    fn best_drop(
        &self,
        anchors: &[Anchor],
        new_id: &mut dyn FnMut() -> String,
    ) -> Option<(KickstandBuildResult, Vec3)> {
        let mut best: Option<(KickstandBuildResult, Vec3, f64)> = None;
        for anchor in anchors {
            let attempts = if self.cleanup { 12 } else { 8 };
            for attempt in 0..attempts {
                let (angle, distance) = self.attempt(attempt);
                if distance > self.max_run {
                    break;
                }
                let Some((build, root_pos)) = self.try_drop(anchor, angle, distance, new_id) else {
                    continue;
                };
                // Either side of a direction covers it; the snuggest root wins.
                let distance_sq =
                    (root_pos.x - self.root_pos.x).powi(2) + (root_pos.y - self.root_pos.y).powi(2);
                if best.as_ref().is_none_or(|b| distance_sq < b.2) {
                    best = Some((build, root_pos, distance_sq));
                }
            }
            if best.is_some() {
                break;
            }
        }
        best.map(|(build, root_pos, _)| (build, root_pos))
    }

    /// The kickstand dropped from `anchor` at `angle`, `distance` out, when
    /// it is legal.
    fn try_drop(
        &self,
        anchor: &Anchor,
        angle: f64,
        distance: f64,
        new_id: &mut dyn FnMut() -> String,
    ) -> Option<(KickstandBuildResult, Vec3)> {
        let model_id = self.trunk.base.model_id.as_str();
        let trunk_root = self.root_pos;
        // The root sits where the trunk's own root would, shifted with the
        // drop's top: the kickstand runs parallel to the trunk.
        let top_x = anchor.pos.x + js_cos(angle) * distance;
        let top_y = anchor.pos.y + js_sin(angle) * distance;
        let mut spot = (
            top_x - (anchor.pos.x - trunk_root.x),
            top_y - (anchor.pos.y - trunk_root.y),
        );
        if let Some(spacing) = self.grid_spacing {
            spot = neighbour_node(self.taken, model_id, trunk_root, spot, spacing)?;
        }

        if !clear_of_trunk(anchor.pos, trunk_root, spot)
            || self.taken.near_a_root(spot.0, spot.1)
            || self
                .roots_placed
                .iter()
                .any(|r| (r.x - spot.0).abs() < 0.1 && (r.y - spot.1).abs() < 0.1)
        {
            return None;
        }
        let root_pos = Vec3::new(spot.0, spot.1, 0.0);
        let top = Vec3::new(top_x, top_y, anchor.pos.z);
        if !self
            .clearance
            .line_clear(top, anchor.pos, anchor.diameter_mm)
            || self.clearance.drop_touches(
                top,
                root_pos,
                js_max(0.001, anchor.diameter_mm / 2.0) + 1.0,
            )
        {
            return None;
        }

        let host = KickstandHostTarget {
            segment_id: anchor.segment_id.clone(),
            support_kind: self.host_kind,
            t: anchor.t,
            pos: anchor.pos,
            diameter_mm: anchor.diameter_mm,
            min_t: None,
        };
        let build = build_kickstand_data(
            &KickstandBuildInput {
                model_id,
                root_pos,
                host: &host,
                layout_overrides: None,
                authored_column_top: None,
                collision_mesh: None,
                max_shaft_angle_deg: None,
            },
            self.live,
            new_id,
        )
        .ok()?;
        self.clearance
            .kickstand_clear(&build)
            .then_some((build, root_pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh::MeshBvh;
    use crate::geom::Matrix4;
    use crate::supports::types::{Joint, Segment, SupportEntity, Transform};

    /// A closed box from `min` to `max`.
    fn block(min: [f32; 3], max: [f32; 3]) -> MeshBvh {
        let corner = |i: usize| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        };
        let faces = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let corners = faces
            .iter()
            .flat_map(|[a, b, c, d]| [*a, *b, *c, *a, *c, *d])
            .map(corner)
            .collect();
        MeshBvh::new(corners, None)
    }

    /// The roots of the kickstands a 20 mm trunk gets with another model
    /// standing just to its +X.
    fn roots_beside_a_neighbour() -> Vec<Vec3> {
        let base = |id: &str| SupportEntity {
            id: id.to_owned(),
            model_id: "own".to_owned(),
            type_id: Some(SupportTypeId::Trunk),
            ..SupportEntity::default()
        };
        let mut segment = Segment::straight("segment", 1.0);
        segment.top_joint = Some(Joint {
            id: "top".to_owned(),
            pos: Vec3::new(0.0, 0.0, 20.0),
            diameter: 1.0,
        });
        let mut state = SupportState::default();
        state.collections.roots.insert(
            "root".to_owned(),
            Roots {
                base: base("root"),
                transform: Transform::default(),
                diameter: 3.0,
                disk_height: 0.5,
                cone_height: 1.0,
                ..Roots::default()
            },
        );
        state.collections.trunks.insert(
            "trunk".to_owned(),
            Trunk {
                base: base("trunk"),
                root_id: "root".to_owned(),
                segments: vec![segment],
                ..Trunk::default()
            },
        );
        let neighbour = block([3.0, -30.0, 0.0], [7.0, 30.0, 30.0]);
        let meshes: BraceMeshes<'_> = [(
            "neighbour".to_owned(),
            BraceMesh {
                bvh: &neighbour,
                matrix: Matrix4::IDENTITY,
            },
        )]
        .into();
        let mut next = 0;
        generate_required_kickstands(
            &state,
            &StabiliserState::default(),
            &AutoBracingSettings::default(),
            &[],
            GridSnap {
                enabled: false,
                spacing_mm: 0.0,
            },
            &meshes,
            &SupportSettings::default(),
            &state,
            &mut || {
                next += 1;
                format!("id-{next}")
            },
        )
        .iter()
        .map(|build| build.root.transform.pos)
        .collect()
    }

    #[test]
    fn a_kickstand_keeps_off_the_model_next_door() {
        let roots = roots_beside_a_neighbour();
        assert!(!roots.is_empty());
        assert!(roots.iter().all(|root| root.x < 2.0), "{roots:?}");
    }
}
